package harness

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"pirtis/internal/llm"
	"pirtis/internal/transcript"
)

// Verdict is the judge's answer on one criterion.
type Verdict struct {
	Criterion string `json:"criterion"`
	Pass      bool   `json:"pass"`
	Reason    string `json:"reason"`
}

const judgeSystem = `You grade one reply from a phone receptionist at a mobile sauna rental company.
You see the call so far and the receptionist's latest reply. Each criterion is a statement about that reply.
Mark it pass if the statement is true of the reply and fail if it is not, with a one-sentence reason.
Judge only the latest reply, using the call for context. Be strict about the substance and lenient about wording.`

// verdictSchema builds the judge's answer shape for these criteria: one
// required property per criterion, named criterion_1, criterion_2, ..., each
// carrying the criterion text as its description. Strict mode then guarantees
// exactly one verdict per criterion. Inside a verdict, reason comes before
// pass so the model writes its reasoning before it commits.
func verdictSchema(criteria []string) json.RawMessage {
	var b strings.Builder
	b.WriteString(`{"type":"object","properties":{`)
	for i, cr := range criteria {
		if i > 0 {
			b.WriteString(",")
		}
		desc, _ := json.Marshal(cr)
		fmt.Fprintf(&b, `"%s":{"type":"object","description":%s,"properties":{"reason":{"type":"string","description":"one sentence"},"pass":{"type":"boolean"}},"required":["reason","pass"],"additionalProperties":false}`,
			criterionKey(i), desc)
	}
	b.WriteString(`},"required":[`)
	for i := range criteria {
		if i > 0 {
			b.WriteString(",")
		}
		fmt.Fprintf(&b, `"%s"`, criterionKey(i))
	}
	b.WriteString(`],"additionalProperties":false}`)
	return json.RawMessage(b.String())
}

func criterionKey(i int) string { return fmt.Sprintf("criterion_%d", i+1) }

// Judge asks the model whether the last reply meets each criterion.
func Judge(ctx context.Context, c llm.Client, tr transcript.Transcript, effects []transcript.Effect, criteria []string) ([]Verdict, error) {
	var b strings.Builder
	b.WriteString("Call so far:\n")
	for _, it := range tr.Items {
		switch it.Type {
		case "message":
			who := "caller"
			if it.Role == "assistant" {
				who = "receptionist"
			}
			fmt.Fprintf(&b, "%s: %s\n", who, it.Content)
		case "function_call":
			fmt.Fprintf(&b, "[receptionist's system called %s with %s]\n", it.Name, it.Arguments)
		case "function_call_output":
			fmt.Fprintf(&b, "[result: %s]\n", it.Output)
		}
	}
	b.WriteString("\nReceptionist's latest reply:\n")
	var said []string
	for _, e := range effects {
		switch e.Type() {
		case "speech":
			said = append(said, fmt.Sprint(e["text"]))
		case "silence":
			said = append(said, "(says nothing, waits)")
		case "transfer":
			said = append(said, "(transfers the call to a person)")
		case "hangup":
			said = append(said, "(hangs up)")
		}
	}
	if len(said) == 0 {
		said = []string{"(nothing)"}
	}
	b.WriteString(strings.Join(said, " "))
	b.WriteString("\n\nCriteria:\n")
	for i, cr := range criteria {
		fmt.Fprintf(&b, "%d. %s\n", i+1, cr)
	}

	zero := 0.0
	resp, err := c.Chat(ctx, llm.Request{
		Messages: []llm.Message{
			{Role: "system", Content: judgeSystem},
			{Role: "user", Content: b.String()},
		},
		Temperature:    &zero,
		ResponseFormat: &llm.ResponseFormat{Name: "verdict", Schema: verdictSchema(criteria), Strict: true},
	})
	if err != nil {
		return nil, err
	}

	var parsed map[string]struct {
		Reason string `json:"reason"`
		Pass   bool   `json:"pass"`
	}
	if err := json.Unmarshal([]byte(resp.Content), &parsed); err != nil {
		return nil, fmt.Errorf("unreadable verdict: %s", truncate(resp.Content, 200))
	}
	out := make([]Verdict, len(criteria))
	missing := 0
	for i, cr := range criteria {
		v, ok := parsed[criterionKey(i)]
		if !ok {
			out[i] = Verdict{Criterion: cr, Pass: false, Reason: "no verdict given"}
			missing++
			continue
		}
		out[i] = Verdict{Criterion: cr, Pass: v.Pass, Reason: v.Reason}
	}
	if missing > 0 {
		return out, fmt.Errorf("verdict missing for %d of %d criteria", missing, len(criteria))
	}
	return out, nil
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "..."
}
