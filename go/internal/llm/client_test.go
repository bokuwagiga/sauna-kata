package llm

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
)

// The request on the wire has the chat completions shape: tools wrapped as
// functions, assistant tool calls with null content, tool results by id,
// and structured output as response_format.json_schema.
func TestOpenAIWireShape(t *testing.T) {
	var got map[string]any
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer k" {
			t.Errorf("missing bearer, got %q", r.Header.Get("Authorization"))
		}
		body, _ := io.ReadAll(r.Body)
		if err := json.Unmarshal(body, &got); err != nil {
			t.Errorf("bad body: %v", err)
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"choices":[{"message":{"content":"{\"ok\":true}","tool_calls":[{"id":"c1","type":"function","function":{"name":"book","arguments":"{}"}}]}}],"usage":{"prompt_tokens":12,"completion_tokens":3}}`))
	}))
	defer srv.Close()

	c := &OpenAI{BaseURL: srv.URL, APIKey: "k", HTTP: srv.Client()}
	resp, err := c.Chat(context.Background(), Request{
		Messages: []Message{
			{Role: "system", Content: "sys"},
			{Role: "assistant", ToolCalls: []ToolCall{{ID: "t1", Name: "book", Arguments: `{"a":1}`}}},
			{Role: "tool", ToolCallID: "t1", Content: `{"ok":true}`},
		},
		Tools:          []ToolSchema{{Name: "book", Description: "d", Parameters: json.RawMessage(`{"type":"object"}`)}},
		ResponseFormat: &ResponseFormat{Name: "verdict", Schema: json.RawMessage(`{"type":"object"}`), Strict: true},
	})
	if err != nil {
		t.Fatal(err)
	}

	if got["model"] != Model {
		t.Errorf("model = %v, want %s", got["model"], Model)
	}
	msgs := got["messages"].([]any)
	if a := msgs[1].(map[string]any); a["content"] != nil || a["tool_calls"].([]any)[0].(map[string]any)["type"] != "function" {
		t.Errorf("assistant tool call message wrong: %v", a)
	}
	if tl := msgs[2].(map[string]any); tl["tool_call_id"] != "t1" || tl["content"] != `{"ok":true}` {
		t.Errorf("tool message wrong: %v", tl)
	}
	tools := got["tools"].([]any)
	if f := tools[0].(map[string]any); f["type"] != "function" || f["function"].(map[string]any)["name"] != "book" {
		t.Errorf("tool wrong: %v", f)
	}
	rf := got["response_format"].(map[string]any)
	if rf["type"] != "json_schema" {
		t.Errorf("response_format.type = %v", rf["type"])
	}
	if js := rf["json_schema"].(map[string]any); js["name"] != "verdict" || js["strict"] != true {
		t.Errorf("json_schema wrong: %v", js)
	}

	if resp.Content != `{"ok":true}` || len(resp.ToolCalls) != 1 || resp.ToolCalls[0].Name != "book" {
		t.Errorf("response parsed wrong: %+v", resp)
	}
	if resp.Usage.PromptTokens != 12 || resp.Usage.CompletionTokens != 3 {
		t.Errorf("usage wrong: %+v", resp.Usage)
	}
}
