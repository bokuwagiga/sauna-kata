package llm

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"pirtis/internal/transcript"
)

func echoTool(name string) Tool {
	return Tool{
		ToolSchema: ToolSchema{Name: name, Description: "echo", Parameters: json.RawMessage(`{"type":"object"}`)},
		Execute: func(_ context.Context, args json.RawMessage) (ToolResult, error) {
			return ToolResult{Data: map[string]any{"got": string(args)}}, nil
		},
	}
}

// The loop runs the tool the model asks for, feeds the result back, and
// returns the text the model then produces, joined with any text said
// alongside the call.
func TestRunAgentToolThenText(t *testing.T) {
	fake := NewFake(
		Response{Content: "One moment.", ToolCalls: []ToolCall{{ID: "t1", Name: "lookup", Arguments: `{"q":1}`}}},
		Response{Content: "Found it."},
	)
	res, err := RunAgent(context.Background(), fake, "sys", nil, []Tool{echoTool("lookup")}, 0)
	if err != nil {
		t.Fatal(err)
	}
	if res.Text != "One moment. Found it." {
		t.Fatalf("text = %q", res.Text)
	}
	if fake.i != 2 {
		t.Fatalf("want 2 model calls, got %d", fake.i)
	}
}

// A terminal tool result stops the loop and becomes the answer.
func TestRunAgentTerminal(t *testing.T) {
	transfer := Tool{
		ToolSchema: ToolSchema{Name: "transfer", Parameters: json.RawMessage(`{"type":"object"}`)},
		Execute: func(context.Context, json.RawMessage) (ToolResult, error) {
			return ToolResult{Terminal: []transcript.Action{transcript.Transfer("front_desk", "asked")}}, nil
		},
	}
	fake := NewFake(Response{ToolCalls: []ToolCall{{ID: "t1", Name: "transfer", Arguments: `{}`}}})
	res, err := RunAgent(context.Background(), fake, "sys", nil, []Tool{transfer}, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Terminal) != 1 || res.Terminal[0].Type() != "transfer" {
		t.Fatalf("terminal = %v", res.Terminal)
	}
	if fake.i != 1 {
		t.Fatalf("loop must stop after the terminal tool, got %d calls", fake.i)
	}
}

// An unknown tool and a failing tool both go back to the model as JSON.
func TestRunAgentToolErrorsGoBack(t *testing.T) {
	failing := Tool{
		ToolSchema: ToolSchema{Name: "book", Parameters: json.RawMessage(`{"type":"object"}`)},
		Execute: func(context.Context, json.RawMessage) (ToolResult, error) {
			return ToolResult{}, errors.New("boom")
		},
	}
	var seen []Message
	fake := &Fake{Script: []Response{
		{ToolCalls: []ToolCall{{ID: "t1", Name: "nope", Arguments: `{}`}, {ID: "t2", Name: "book", Arguments: `{}`}}},
	}}
	fake.Default = Response{Content: "ok"}
	spy := clientFunc(func(ctx context.Context, req Request) (Response, error) {
		seen = req.Messages
		return fake.Chat(ctx, req)
	})
	if _, err := RunAgent(context.Background(), spy, "sys", nil, []Tool{failing}, 0); err != nil {
		t.Fatal(err)
	}
	var toolMsgs []string
	for _, m := range seen {
		if m.Role == "tool" {
			toolMsgs = append(toolMsgs, m.Content)
		}
	}
	if len(toolMsgs) != 2 || !strings.Contains(toolMsgs[0], "unknown tool") || !strings.Contains(toolMsgs[1], "TOOL_FAILED") {
		t.Fatalf("tool messages = %v", toolMsgs)
	}
}

// A model that never stops calling tools hits the cap and the caller hears
// about it.
func TestRunAgentMaxCalls(t *testing.T) {
	calls := 0
	forever := clientFunc(func(context.Context, Request) (Response, error) {
		calls++
		return Response{ToolCalls: []ToolCall{{ID: "t", Name: "lookup", Arguments: `{}`}}}, nil
	})
	_, err := RunAgent(context.Background(), forever, "sys", nil, []Tool{echoTool("lookup")}, 3)
	if !errors.Is(err, ErrMaxCalls) {
		t.Fatalf("want ErrMaxCalls, got %v", err)
	}
	if calls != 3 {
		t.Fatalf("want exactly 3 calls, got %d", calls)
	}
}

type clientFunc func(ctx context.Context, req Request) (Response, error)

func (f clientFunc) Chat(ctx context.Context, req Request) (Response, error) { return f(ctx, req) }
