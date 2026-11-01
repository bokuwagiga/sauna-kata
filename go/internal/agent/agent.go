// Package agent is the receptionist. This is where the work happens.
//
// Decide gets the call so far and returns what to do next. Handlers, in
// actions.go, carry those actions out. Everything else in the repo is
// plumbing you can use or ignore.
package agent

import (
	"context"
	"encoding/json"

	"pirtis/internal/api"
	"pirtis/internal/llm"
	"pirtis/internal/transcript"
)

// Decide returns the receptionist's next actions given the transcript so far.
//
// What is here now is a starting point. It passes the happy-path fixture and
// not much else.
func Decide(ctx context.Context, t transcript.Transcript, a api.Client, c llm.Client) ([]transcript.Action, error) {
	units, err := a.ListUnits(ctx)
	if err != nil {
		return nil, err
	}
	raw, _ := json.Marshal(units)
	system := "You are the receptionist for Pirtis on Wheels, a mobile sauna rental. " +
		"Help the caller book a sauna. Today is " + t.Today + ". " +
		"Units: " + string(raw)

	// every item becomes history, whether the caller finished the sentence or not
	var history []llm.Message
	for _, it := range t.Items {
		switch it.Type {
		case "message":
			history = append(history, llm.Message{Role: it.Role, Content: it.Content})
		case "function_call":
			history = append(history, llm.Message{Role: "assistant", Content: "[called " + it.Name + " with " + it.Arguments + "]"})
		case "function_call_output":
			history = append(history, llm.Message{Role: "assistant", Content: "[result: " + it.Output + "]"})
		}
	}

	tools := []llm.Tool{
		{
			ToolSchema: llm.ToolSchema{
				Name:        "availability",
				Description: "Free dates per unit between two dates, YYYY-MM-DD.",
				Parameters:  json.RawMessage(`{"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"}},"required":["from","to"]}`),
			},
			// no people, no weight, no power. every unit comes back and the model picks.
			Execute: func(ctx context.Context, args json.RawMessage) (llm.ToolResult, error) {
				var q api.AvailabilityQuery
				if err := json.Unmarshal(args, &q); err != nil {
					return llm.ToolResult{}, err
				}
				out, err := a.Availability(ctx, api.AvailabilityQuery{From: q.From, To: q.To})
				return llm.ToolResult{Data: out}, err
			},
		},
		{
			ToolSchema: llm.ToolSchema{
				Name:        "book",
				Description: "Create a booking.",
				Parameters:  json.RawMessage(`{"type":"object","properties":{"customer_id":{"type":"string"},"unit_id":{"type":"string"},"start":{"type":"string"},"end":{"type":"string"},"people":{"type":"integer"}},"required":["customer_id","unit_id","start","end","people"]}`),
			},
			Execute: func(ctx context.Context, args json.RawMessage) (llm.ToolResult, error) {
				var req api.BookingRequest
				if err := json.Unmarshal(args, &req); err != nil {
					return llm.ToolResult{}, err
				}
				req.Site.PowerAvailable = true // nobody asked
				req.TowCapacityKg = 3500       // nobody asked
				b, err := a.CreateBooking(ctx, req)
				return llm.ToolResult{Data: b}, err
			},
		},
	}

	res, err := llm.RunAgent(ctx, c, system, history, tools, 8)
	if err != nil {
		return nil, err
	}
	if res.Terminal != nil {
		return res.Terminal, nil
	}
	return []transcript.Action{transcript.Say(res.Text, "neutral", 1.0)}, nil
}
