package llm

import (
	"context"
	"encoding/json"
	"errors"
	"strings"

	"pirtis/internal/api"
	"pirtis/internal/transcript"
)

// ToolResult is what a tool hands back. Set Data to feed something to the
// model and keep going. Set Terminal to stop the loop and make those actions
// the answer.
type ToolResult struct {
	Data     any
	Terminal []transcript.Action
}

// Tool is a schema the model sees plus the code that runs when it calls it.
type Tool struct {
	ToolSchema
	Execute func(ctx context.Context, args json.RawMessage) (ToolResult, error)
}

// Result is what RunAgent returns: the model's text, or terminal actions from
// a tool.
type Result struct {
	Text     string
	Terminal []transcript.Action
}

// ErrMaxCalls is returned when the model is still asking for tools after
// maxCalls rounds. Result.Text holds whatever it said along the way.
var ErrMaxCalls = errors.New("llm: tool loop hit the call limit")

// RunAgent runs the tool loop: call the model, run whatever tools it asks
// for, feed the results back, repeat until it answers in text or a tool
// returns Terminal. maxCalls caps the number of model calls; 0 means 8, and
// hitting the cap returns ErrMaxCalls.
//
// A tool the model names but that does not exist, and a tool that returns an
// error, both go back to the model as JSON rather than stopping the loop.
//
// Text the model produces alongside a tool call ("one moment") is kept and
// joined to what it says after, since on a phone that is one reply.
func RunAgent(ctx context.Context, c Client, system string, history []Message, tools []Tool, maxCalls int) (Result, error) {
	if maxCalls <= 0 {
		maxCalls = 8
	}
	msgs := make([]Message, 0, len(history)+1)
	msgs = append(msgs, Message{Role: "system", Content: system})
	msgs = append(msgs, history...)

	byName := make(map[string]Tool, len(tools))
	schemas := make([]ToolSchema, 0, len(tools))
	for _, t := range tools {
		byName[t.Name] = t
		schemas = append(schemas, t.ToolSchema)
	}

	var text string
	for i := 0; i < maxCalls; i++ {
		reply, err := c.Chat(ctx, Request{Messages: msgs, Tools: schemas})
		if err != nil {
			return Result{}, err
		}
		if reply.Content != "" {
			text = strings.TrimSpace(text + " " + reply.Content)
		}
		if len(reply.ToolCalls) == 0 {
			return Result{Text: text}, nil
		}

		msgs = append(msgs, Message{Role: "assistant", Content: reply.Content, ToolCalls: reply.ToolCalls})
		for _, call := range reply.ToolCalls {
			var res ToolResult
			if t, ok := byName[call.Name]; ok {
				var err error
				if res, err = t.Execute(ctx, json.RawMessage(call.Arguments)); err != nil {
					res = ToolResult{Data: map[string]any{"error": ErrorCode(err), "detail": err.Error()}}
				}
			} else {
				res = ToolResult{Data: map[string]any{"error": "unknown tool " + call.Name}}
			}
			if res.Terminal != nil {
				return Result{Text: text, Terminal: res.Terminal}, nil
			}
			b, _ := json.Marshal(res.Data)
			msgs = append(msgs, Message{Role: "tool", ToolCallID: call.ID, Content: string(b)})
		}
	}
	return Result{Text: text}, ErrMaxCalls
}

// ErrorCode is the API reason code when err is an *api.Error, else TOOL_FAILED.
func ErrorCode(err error) string {
	var e *api.Error
	if errors.As(err, &e) {
		return e.Code
	}
	return "TOOL_FAILED"
}
