// Command harness runs the fixtures against the agent.
//
//	go run ./cmd/harness                 all fixtures
//	go run ./cmd/harness -id silence     one fixture
//	go run ./cmd/harness -runs 3 -need 2 three times each, pass on two, for a stability check
//	go run ./cmd/harness -v              print actions, effects and verdicts per run
//	go run ./cmd/harness -trace runs     also write every LLM call and per-run results under runs/
package main

import (
	"bufio"
	"context"
	"flag"
	"fmt"
	"os"
	"strings"
	"time"

	"pirtis/internal/agent"
	"pirtis/internal/harness"
	"pirtis/internal/llm"
)

func main() {
	fixturesDir := flag.String("fixtures", "../fixtures", "directory with *.yaml fixtures and seeds/")
	trace := flag.String("trace", "", "directory to write every LLM call and per-run results to; off by default")
	ids := flag.String("id", "", "comma-separated fixture ids to run; default all")
	runs := flag.Int("runs", 1, "runs per fixture")
	need := flag.Int("need", 1, "runs that must pass for the fixture to pass")
	verbose := flag.Bool("v", false, "print actions, effects and verdicts per run")
	flag.Parse()

	loadDotEnv(".env")

	client, err := llm.NewOpenAIFromEnv()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		fmt.Fprintln(os.Stderr, "set LLM_API_KEY, and optionally LLM_BASE_URL; see .env.example")
		os.Exit(2)
	}

	fixtures, err := harness.LoadFixtures(*fixturesDir)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	if *ids != "" {
		want := map[string]bool{}
		for _, id := range strings.Split(*ids, ",") {
			want[strings.TrimSpace(id)] = true
		}
		var kept []harness.Fixture
		for _, f := range fixtures {
			if want[f.ID] {
				kept = append(kept, f)
			}
		}
		fixtures = kept
	}
	if len(fixtures) == 0 {
		fmt.Fprintln(os.Stderr, "no fixtures matched")
		os.Exit(2)
	}

	runner := &harness.Runner{
		LLM:      client,
		Decide:   agent.Decide,
		Handlers: agent.Handlers,
		TraceDir: *trace,
	}

	ctx := context.Background()
	passed := 0
	fmt.Printf("%-26s %-4s %-6s %6s %14s %8s %8s\n", "fixture", "run", "result", "calls", "tokens in/out", "llm", "wall")
	for _, f := range fixtures {
		fr := runner.Run(ctx, f, *runs, *need)
		for _, r := range fr.Runs {
			result := "PASS"
			if !r.Passed() {
				result = "FAIL"
			}
			fmt.Printf("%-26s %-4d %-6s %6d %14s %8s %8s\n",
				f.ID, r.Index, result, r.Stats.Calls,
				fmt.Sprintf("%d/%d", r.Stats.PromptTokens, r.Stats.CompletionTokens),
				r.Stats.Duration.Round(10*time.Millisecond), r.Wall.Round(10*time.Millisecond))
			if r.Err != nil {
				fmt.Printf("    error: %v\n", r.Err)
			}
			for _, fl := range r.Failures {
				fmt.Printf("    - %s\n", fl)
			}
			if *verbose {
				for _, a := range r.Actions {
					fmt.Printf("    action: %v\n", a)
				}
				for _, e := range r.Effects {
					fmt.Printf("    effect: %v\n", e)
				}
				for _, v := range r.Verdicts {
					fmt.Printf("    judge: %v  %s: %s\n", v.Pass, v.Criterion, v.Reason)
				}
			}
		}
		if fr.Passed() {
			passed++
			fmt.Printf("%-26s      pass  (%d/%d, need %d)\n", "", fr.PassCount(), len(fr.Runs), fr.Threshold)
		} else {
			fmt.Printf("%-26s      FAIL  (%d/%d, need %d)\n", "", fr.PassCount(), len(fr.Runs), fr.Threshold)
		}
	}
	fmt.Printf("\n%d/%d fixtures passed\n", passed, len(fixtures))
	if passed < len(fixtures) {
		os.Exit(1)
	}
}

// loadDotEnv sets KEY=VALUE lines from path into the environment when the
// variable is not already set. A missing file is fine.
func loadDotEnv(path string) {
	f, err := os.Open(path)
	if err != nil {
		return
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		k, v, ok := strings.Cut(line, "=")
		if !ok {
			continue
		}
		k = strings.TrimSpace(k)
		v = strings.Trim(strings.TrimSpace(v), `"'`)
		if os.Getenv(k) == "" {
			_ = os.Setenv(k, v)
		}
	}
}
