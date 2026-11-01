package harness

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"pirtis/internal/agent"
	"pirtis/internal/api"
	"pirtis/internal/llm"
	"pirtis/internal/transcript"
)

// DecideFunc is the agent's entry point.
type DecideFunc func(ctx context.Context, t transcript.Transcript, a api.Client, c llm.Client) ([]transcript.Action, error)

// Runner runs fixtures against one agent.
type Runner struct {
	LLM      llm.Client
	Judge    llm.Client // grades `judge` criteria; nil means LLM
	Decide   DecideFunc
	Handlers map[string]agent.Handler
	TraceDir string // where LLM calls and per-run results are written; "" for nowhere
}

// RunResult is one execution of one fixture: one call to Decide, the
// handlers, the checks.
type RunResult struct {
	Index    int
	Actions  []transcript.Action
	Effects  []transcript.Effect
	Failures []string
	Verdicts []Verdict
	Err      error
	Stats    llm.Stats // the agent's calls only; the judge is counted separately
	Wall     time.Duration
	Calls    []api.Call
}

// Passed is true when the run produced no failures and no error.
func (r RunResult) Passed() bool { return r.Err == nil && len(r.Failures) == 0 }

// FixtureResult is every run of one fixture.
type FixtureResult struct {
	Fixture   Fixture
	Runs      []RunResult
	Threshold int // runs that must pass
}

// PassCount is how many runs passed.
func (f FixtureResult) PassCount() int {
	n := 0
	for _, r := range f.Runs {
		if r.Passed() {
			n++
		}
	}
	return n
}

// Passed is true when at least Threshold runs passed.
func (f FixtureResult) Passed() bool { return f.PassCount() >= f.Threshold }

// Run executes one fixture `runs` times; `need` of them must pass.
func (r *Runner) Run(ctx context.Context, f Fixture, runs, need int) FixtureResult {
	if runs <= 0 {
		runs = 1
	}
	if need <= 0 || need > runs {
		need = runs
	}
	out := FixtureResult{Fixture: f, Threshold: need}
	for i := 1; i <= runs; i++ {
		out.Runs = append(out.Runs, r.runOnce(ctx, f, i))
	}
	return out
}

func (r *Runner) runOnce(ctx context.Context, f Fixture, i int) RunResult {
	res := RunResult{Index: i}

	seed, err := api.LoadSeed(f.SeedPath())
	if err != nil {
		res.Err = fmt.Errorf("seed: %w", err)
		return res
	}
	for _, sb := range f.Setup.Bookings {
		seed.Bookings = append(seed.Bookings, sb.Booking())
	}
	mem := api.NewMemory(seed, f.Faults)
	before := mem.Bookings()

	tr := f.Transcript
	tr.Items = append([]transcript.Item(nil), tr.Items...)

	dir := ""
	if r.TraceDir != "" {
		dir = filepath.Join(r.TraceDir, f.ID, fmt.Sprintf("run-%d", i))
	}
	rec := llm.NewRecorder(r.LLM, dir)

	start := time.Now()
	actions, err := r.Decide(ctx, tr, mem, rec)
	res.Wall = time.Since(start)
	res.Stats = rec.Stats()
	res.Actions = actions
	if err != nil {
		res.Err = fmt.Errorf("decide: %w", err)
		res.Calls = mem.Log()
		r.writeResult(dir, f, res, mem.Bookings())
		return res
	}

	hc := agent.HandlerContext{API: mem, Transcript: tr}
	for _, a := range actions {
		h, ok := r.Handlers[a.Type()]
		if !ok {
			res.Failures = append(res.Failures, fmt.Sprintf("no handler for action type %q", a.Type()))
			continue
		}
		ef, err := h(ctx, a, hc)
		if err != nil {
			res.Failures = append(res.Failures, fmt.Sprintf("handler %q: %v", a.Type(), err))
			continue
		}
		res.Effects = append(res.Effects, ef)
	}

	res.Calls = mem.Log()
	after := mem.Bookings()
	res.Failures = append(res.Failures, Evaluate(f.Expect, World{
		Effects:    res.Effects,
		Calls:      res.Calls,
		Before:     before,
		After:      after,
		Identified: identifiedIn(tr),
	})...)

	if len(f.Judge) > 0 {
		judge := r.Judge
		if judge == nil {
			judge = r.LLM
		}
		jdir := ""
		if dir != "" {
			jdir = filepath.Join(dir, "judge")
		}
		verdicts, err := Judge(ctx, llm.NewRecorder(judge, jdir), tr, res.Effects, f.Judge)
		res.Verdicts = verdicts
		if err != nil {
			res.Failures = append(res.Failures, "judge: "+err.Error())
		}
		for _, v := range verdicts {
			if !v.Pass {
				res.Failures = append(res.Failures, fmt.Sprintf("judge: %s: %s", v.Criterion, v.Reason))
			}
		}
	}

	r.writeResult(dir, f, res, after)
	return res
}

// identifiedIn returns the customer ids a successful customers.identify in
// the transcript returned.
func identifiedIn(tr transcript.Transcript) map[string]bool {
	names := map[string]string{}
	out := map[string]bool{}
	for _, it := range tr.Items {
		switch it.Type {
		case "function_call":
			names[it.CallID] = it.Name
		case "function_call_output":
			if names[it.CallID] != api.MethodCustomersIdentify {
				continue
			}
			var r api.IdentifyResult
			if json.Unmarshal([]byte(it.Output), &r) == nil && r.OK && r.CustomerID != "" {
				out[r.CustomerID] = true
			}
		}
	}
	return out
}

func (r *Runner) writeResult(dir string, f Fixture, res RunResult, bookings []api.Booking) {
	if dir == "" {
		return
	}
	type out struct {
		Fixture  string              `json:"fixture"`
		Run      int                 `json:"run"`
		Passed   bool                `json:"passed"`
		Error    string              `json:"error,omitempty"`
		Failures []string            `json:"failures,omitempty"`
		Verdicts []Verdict           `json:"verdicts,omitempty"`
		Actions  []transcript.Action `json:"actions"`
		Effects  []transcript.Effect `json:"effects"`
		Calls    []api.Call          `json:"api_calls"`
		Bookings []api.Booking       `json:"bookings"`
		LLM      llm.Stats           `json:"llm"`
		WallMs   int64               `json:"wall_ms"`
	}
	o := out{Fixture: f.ID, Run: res.Index, Passed: res.Passed(), Failures: res.Failures, Verdicts: res.Verdicts,
		Actions: res.Actions, Effects: res.Effects, Calls: res.Calls, Bookings: bookings,
		LLM: res.Stats, WallMs: res.Wall.Milliseconds()}
	if res.Err != nil {
		o.Error = res.Err.Error()
	}
	b, err := json.MarshalIndent(o, "", "  ")
	if err != nil {
		return
	}
	_ = os.MkdirAll(dir, 0o755)
	_ = os.WriteFile(filepath.Join(dir, "result.json"), b, 0o644)
}
