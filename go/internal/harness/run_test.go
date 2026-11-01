package harness

import (
	"context"
	"os"
	"strings"
	"testing"

	"pirtis/internal/agent"
	"pirtis/internal/api"
	"pirtis/internal/llm"
	"pirtis/internal/transcript"
)

// These tests drive the harness with stub agents, never with the shipped
// agent, so a candidate's rewrite of internal/agent cannot break them.

const fixturesDir = "../../../fixtures"

func load(t *testing.T, id string) Fixture {
	t.Helper()
	fs, err := LoadFixtures(fixturesDir)
	if err != nil {
		t.Fatal(err)
	}
	for _, f := range fs {
		if f.ID == id {
			return f
		}
	}
	t.Fatalf("no fixture %q", id)
	return Fixture{}
}

// handlers is the effect contract the harness checks against: say -> speech,
// wait -> silence, transfer -> transfer, end_call -> hangup.
var handlers = map[string]agent.Handler{
	"say": func(_ context.Context, a transcript.Action, _ agent.HandlerContext) (transcript.Effect, error) {
		return transcript.Effect{"type": "speech", "text": a["text"]}, nil
	},
	"wait": func(_ context.Context, a transcript.Action, _ agent.HandlerContext) (transcript.Effect, error) {
		return transcript.Effect{"type": "silence", "seconds": a["seconds"]}, nil
	},
	"transfer": func(_ context.Context, a transcript.Action, _ agent.HandlerContext) (transcript.Effect, error) {
		return transcript.Effect{"type": "transfer", "to": a["to"]}, nil
	},
	"end_call": func(_ context.Context, a transcript.Action, _ agent.HandlerContext) (transcript.Effect, error) {
		return transcript.Effect{"type": "hangup"}, nil
	},
}

// passJudge passes every criterion.
var passJudge = &llm.Fake{Default: llm.Response{Content: `{"criterion_1":{"reason":"ok","pass":true},"criterion_2":{"reason":"ok","pass":true},"criterion_3":{"reason":"ok","pass":true},"criterion_4":{"reason":"ok","pass":true}}`}}

func runner(decide DecideFunc) *Runner {
	return &Runner{LLM: llm.NewFake(), Judge: passJudge, Decide: decide, Handlers: handlers}
}

func says(text string) DecideFunc {
	return func(context.Context, transcript.Transcript, api.Client, llm.Client) ([]transcript.Action, error) {
		return []transcript.Action{transcript.Say(text, "neutral", 1)}, nil
	}
}

func TestAllFixturesLoad(t *testing.T) {
	fs, err := LoadFixtures(fixturesDir)
	if err != nil {
		t.Fatal(err)
	}
	if len(fs) < 10 {
		t.Fatalf("only %d fixtures", len(fs))
	}
	for _, f := range fs {
		if _, err := os.Stat(f.SeedPath()); err != nil {
			t.Errorf("%s: %v", f.ID, err)
		}
		last := f.Transcript.Items[len(f.Transcript.Items)-1]
		if last.Type != "message" || last.End <= 0 || f.Transcript.Now < last.End {
			t.Errorf("%s: last item should be a timed message before now, got %+v (now %v)", f.ID, last, f.Transcript.Now)
		}
	}
}

// An agent that books what the caller confirmed passes the happy path: the
// diff sees the booking, the invariants hold, no model needed.
func TestHappyPathBooking(t *testing.T) {
	book := func(ctx context.Context, tr transcript.Transcript, a api.Client, _ llm.Client) ([]transcript.Action, error) {
		_, err := a.CreateBooking(ctx, api.BookingRequest{CustomerID: "cus_07", UnitID: "unit_mazoji",
			Start: "2026-10-06", End: "2026-10-07", People: 4, Site: api.Site{PowerAvailable: true}, TowCapacityKg: 2000})
		if err != nil {
			return nil, err
		}
		return []transcript.Action{transcript.Say("Done, see you Tuesday.", "warm", 1)}, nil
	}
	res := runner(book).Run(context.Background(), load(t, "happy-path"), 1, 1)
	r := res.Runs[0]
	if !res.Passed() {
		t.Fatalf("happy path failed: %v (err %v)", r.Failures, r.Err)
	}
	if r.Stats.Calls != 0 {
		t.Fatalf("no model was used, got %d calls", r.Stats.Calls)
	}
	if res.Threshold != 1 {
		t.Fatalf("one run means a threshold of one, got %d", res.Threshold)
	}
}

// The interim fixture wants silence: speech fails it, a wait passes it.
func TestInterimWantsWait(t *testing.T) {
	res := runner(says("Take your time!")).Run(context.Background(), load(t, "interim-speech"), 1, 1)
	if res.Passed() {
		t.Fatal("speech must not pass interim-speech")
	}
	if !strings.Contains(strings.Join(res.Runs[0].Failures, "\n"), "wanted wait") {
		t.Fatalf("failure should say wait was wanted: %v", res.Runs[0].Failures)
	}
	wait := func(context.Context, transcript.Transcript, api.Client, llm.Client) ([]transcript.Action, error) {
		return []transcript.Action{transcript.Wait(1.5)}, nil
	}
	if res := runner(wait).Run(context.Background(), load(t, "interim-speech"), 1, 1); !res.Passed() {
		t.Fatalf("wait should pass: %v", res.Runs[0].Failures)
	}
}

// A 422 from the API is a failure even if the reply talks its way out, and
// the failure line says what it was.
func TestRuleViolationIsCaught(t *testing.T) {
	wrong := func(ctx context.Context, tr transcript.Transcript, a api.Client, _ llm.Client) ([]transcript.Action, error) {
		_, _ = a.CreateBooking(ctx, api.BookingRequest{CustomerID: "cus_19", UnitID: "unit_mazoji",
			Start: "2026-10-03", End: "2026-10-04", People: 7, Site: api.Site{PowerAvailable: true}, TowCapacityKg: 3500})
		return []transcript.Action{transcript.Say("Sorry, that one is too small. Didžioji then?", "neutral", 1)}, nil
	}
	res := runner(wrong).Run(context.Background(), load(t, "capacity"), 1, 1)
	if res.Passed() {
		t.Fatal("a CAPACITY_EXCEEDED in the log must fail the fixture")
	}
	joined := strings.Join(res.Runs[0].Failures, "\n")
	if !strings.Contains(joined, "CAPACITY_EXCEEDED") || !strings.Contains(joined, "rule violation") {
		t.Fatalf("failure should name the code and the meaning: %v", res.Runs[0].Failures)
	}
}

// A booking nobody asked for fails a fixture that expects no change.
func TestUnexpectedBookingIsCaught(t *testing.T) {
	eager := func(ctx context.Context, tr transcript.Transcript, a api.Client, _ llm.Client) ([]transcript.Action, error) {
		_, err := a.CreateBooking(ctx, api.BookingRequest{CustomerID: "cus_12", UnitID: "unit_didzioji",
			Start: "2026-10-10", End: "2026-10-11", People: 6, Site: api.Site{PowerAvailable: true}, TowCapacityKg: 2500})
		if err != nil {
			return nil, err
		}
		return []transcript.Action{transcript.Say("Booked the big one.", "neutral", 1)}, nil
	}
	res := runner(eager).Run(context.Background(), load(t, "ask-tow-capacity"), 1, 1)
	if res.Passed() {
		t.Fatal("guessing a towing capacity and booking must fail")
	}
	if !strings.Contains(strings.Join(res.Runs[0].Failures, "\n"), "unexpected created") {
		t.Fatalf("failure should report the unexpected booking: %v", res.Runs[0].Failures)
	}
}

// Reading another customer's record is a failure; reading the caller's is
// fine. The silence fixture has the caller identified and expects speech and
// no change, so only the invariant decides.
func TestIdentityInvariant(t *testing.T) {
	leak := func(ctx context.Context, tr transcript.Transcript, a api.Client, _ llm.Client) ([]transcript.Action, error) {
		_, _ = a.GetCustomer(ctx, "cus_31")
		return []transcript.Action{transcript.Say("Still there?", "neutral", 1)}, nil
	}
	if res := runner(leak).Run(context.Background(), load(t, "silence"), 1, 1); res.Passed() {
		t.Fatal("customers.get for someone else must fail")
	}
	self := func(ctx context.Context, tr transcript.Transcript, a api.Client, _ llm.Client) ([]transcript.Action, error) {
		_, _ = a.GetCustomer(ctx, "cus_12")
		return []transcript.Action{transcript.Say("Still there?", "neutral", 1)}, nil
	}
	if res := runner(self).Run(context.Background(), load(t, "silence"), 1, 1); !res.Passed() {
		t.Fatalf("customers.get for the identified caller must pass: %v", res.Runs[0].Failures)
	}
}

// A failing verdict fails the run with the reason on the line, and judge
// calls do not count against the agent.
func TestJudgeFailureIsReported(t *testing.T) {
	failJudge := &llm.Fake{Default: llm.Response{Content: `{"criterion_1":{"reason":"it answered the question instead of asking for a name","pass":false}}`}}
	r := runner(says("Yes, Wednesday is still on."))
	r.Judge = failJudge
	f := load(t, "identify-before-reading")
	f.Judge = []string{"The reply asks the caller for their name."}
	res := r.Run(context.Background(), f, 1, 1)
	if res.Passed() {
		t.Fatal("a failing verdict must fail the run")
	}
	found := false
	for _, fl := range res.Runs[0].Failures {
		if strings.HasPrefix(fl, "judge: ") && strings.Contains(fl, "instead of asking") {
			found = true
		}
	}
	if !found {
		t.Fatalf("want a judge failure with the reason, got %v", res.Runs[0].Failures)
	}
	if res.Runs[0].Stats.Calls != 0 {
		t.Fatalf("judge calls must not count against the agent, got %d", res.Runs[0].Stats.Calls)
	}
}

// An action type nobody registered a handler for fails the run, and a handler
// someone adds makes it pass.
func TestUnknownActionNeedsHandler(t *testing.T) {
	decide := func(context.Context, transcript.Transcript, api.Client, llm.Client) ([]transcript.Action, error) {
		return []transcript.Action{{"type": "hold_tone", "seconds": 2.0}, transcript.Say("still there?", "warm", 1)}, nil
	}
	r := runner(decide)
	if res := r.Run(context.Background(), load(t, "silence"), 1, 1); res.Passed() {
		t.Fatal("hold_tone has no handler, run must fail")
	}
	withHold := map[string]agent.Handler{}
	for k, v := range handlers {
		withHold[k] = v
	}
	withHold["hold_tone"] = func(context.Context, transcript.Action, agent.HandlerContext) (transcript.Effect, error) {
		return transcript.Effect{"type": "hold_tone"}, nil
	}
	r.Handlers = withHold
	if res := r.Run(context.Background(), load(t, "silence"), 1, 1); !res.Passed() {
		t.Fatalf("with a handler the silence fixture should pass: %v", res.Runs[0].Failures)
	}
}
