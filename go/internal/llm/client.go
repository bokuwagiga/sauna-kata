// Package llm is a non-streaming chat client in the OpenAI message format,
// plus a small tool loop. A Recorder counts calls and tokens, and writes every
// request and response to disk when the harness is run with -trace.
package llm

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Message is one chat message. The system prompt is a Message with Role
// "system", by convention the first one.
type Message struct {
	Role       string     `json:"role"`                   // system | user | assistant | tool
	Content    string     `json:"content,omitempty"`      // text
	ToolCalls  []ToolCall `json:"tool_calls,omitempty"`   // assistant only
	ToolCallID string     `json:"tool_call_id,omitempty"` // tool only
}

// ToolCall is the model asking for a tool to run.
type ToolCall struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	Arguments string `json:"arguments"` // JSON text
}

// ToolSchema tells the model what a tool is and what it takes.
type ToolSchema struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	Parameters  json.RawMessage `json:"parameters"` // JSON schema object
}

// ResponseFormat asks for structured output: the reply is JSON matching
// Schema. Schema is a JSON Schema object. With Strict the model is held to
// it exactly, which requires additionalProperties: false and every property
// listed as required.
type ResponseFormat struct {
	Name        string          `json:"name"`
	Description string          `json:"description,omitempty"`
	Schema      json.RawMessage `json:"schema"`
	Strict      bool            `json:"strict,omitempty"`
}

// Request is one chat completion request.
type Request struct {
	Messages       []Message       `json:"messages"`
	Tools          []ToolSchema    `json:"tools,omitempty"`
	Temperature    *float64        `json:"temperature,omitempty"`
	ResponseFormat *ResponseFormat `json:"response_format,omitempty"`
}

// Usage is what the call cost in tokens.
type Usage struct {
	PromptTokens     int `json:"prompt_tokens"`
	CompletionTokens int `json:"completion_tokens"`
}

// Response is what came back: text, tool calls, or both.
type Response struct {
	Content   string     `json:"content,omitempty"`
	ToolCalls []ToolCall `json:"tool_calls,omitempty"`
	Usage     Usage      `json:"usage"`
}

// Client is anything that can answer a chat request.
type Client interface {
	Chat(ctx context.Context, req Request) (Response, error)
}

// Model is the one model the kata runs on. Everyone is graded against the
// same model, so it is not configurable.
const Model = "gpt-4o-mini"

// OpenAI talks to an OpenAI-compatible chat completions endpoint.
type OpenAI struct {
	BaseURL string
	APIKey  string
	HTTP    *http.Client
}

// NewOpenAIFromEnv reads LLM_API_KEY (required) and LLM_BASE_URL (optional).
func NewOpenAIFromEnv() (*OpenAI, error) {
	key := os.Getenv("LLM_API_KEY")
	if key == "" {
		return nil, fmt.Errorf("LLM_API_KEY is not set")
	}
	base := os.Getenv("LLM_BASE_URL")
	if base == "" {
		base = "https://api.openai.com/v1"
	}
	return &OpenAI{
		BaseURL: strings.TrimRight(base, "/"),
		APIKey:  key,
		HTTP:    &http.Client{Timeout: 60 * time.Second},
	}, nil
}

type wireFunction struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

type wireToolCall struct {
	ID       string       `json:"id"`
	Type     string       `json:"type"`
	Function wireFunction `json:"function"`
}

type wireMessage struct {
	Role       string         `json:"role"`
	Content    *string        `json:"content"`
	ToolCalls  []wireToolCall `json:"tool_calls,omitempty"`
	ToolCallID string         `json:"tool_call_id,omitempty"`
}

type wireTool struct {
	Type     string `json:"type"`
	Function struct {
		Name        string          `json:"name"`
		Description string          `json:"description"`
		Parameters  json.RawMessage `json:"parameters"`
	} `json:"function"`
}

type wireResponseFormat struct {
	Type       string          `json:"type"` // "json_schema"
	JSONSchema *ResponseFormat `json:"json_schema,omitempty"`
}

type wireRequest struct {
	Model          string              `json:"model"`
	Messages       []wireMessage       `json:"messages"`
	Tools          []wireTool          `json:"tools,omitempty"`
	Temperature    *float64            `json:"temperature,omitempty"`
	ResponseFormat *wireResponseFormat `json:"response_format,omitempty"`
}

type wireResponse struct {
	Choices []struct {
		Message struct {
			Content   *string        `json:"content"`
			ToolCalls []wireToolCall `json:"tool_calls"`
		} `json:"message"`
		FinishReason string `json:"finish_reason"`
	} `json:"choices"`
	Usage Usage `json:"usage"`
	Error *struct {
		Message string `json:"message"`
		Type    string `json:"type"`
	} `json:"error"`
}

// Chat implements Client. Rate limits, server errors and network errors are
// retried three times with a short backoff, honouring Retry-After.
func (o *OpenAI) Chat(ctx context.Context, req Request) (Response, error) {
	var last error
	for attempt := 0; attempt < 3; attempt++ {
		if attempt > 0 {
			wait := time.Duration(1<<attempt) * time.Second
			var re *retryableError
			if errorsAs(last, &re) && re.after > 0 {
				wait = re.after
			}
			select {
			case <-ctx.Done():
				return Response{}, ctx.Err()
			case <-time.After(wait):
			}
		}
		resp, err := o.chatOnce(ctx, req)
		if err == nil {
			return resp, nil
		}
		last = err
		var re *retryableError
		if !errorsAs(err, &re) {
			return Response{}, err
		}
	}
	return Response{}, last
}

// retryableError is a 429, a 5xx, or a failed connection.
type retryableError struct {
	err   error
	after time.Duration
}

func (e *retryableError) Error() string { return e.err.Error() }
func (e *retryableError) Unwrap() error { return e.err }

func errorsAs(err error, target **retryableError) bool {
	for err != nil {
		if re, ok := err.(*retryableError); ok {
			*target = re
			return true
		}
		u, ok := err.(interface{ Unwrap() error })
		if !ok {
			return false
		}
		err = u.Unwrap()
	}
	return false
}

func (o *OpenAI) chatOnce(ctx context.Context, req Request) (Response, error) {
	wr := wireRequest{Model: Model, Temperature: req.Temperature}
	if req.ResponseFormat != nil {
		wr.ResponseFormat = &wireResponseFormat{Type: "json_schema", JSONSchema: req.ResponseFormat}
	}
	for _, m := range req.Messages {
		wm := wireMessage{Role: m.Role, ToolCallID: m.ToolCallID}
		if m.Content != "" || len(m.ToolCalls) == 0 {
			c := m.Content
			wm.Content = &c
		}
		for _, tc := range m.ToolCalls {
			wm.ToolCalls = append(wm.ToolCalls, wireToolCall{ID: tc.ID, Type: "function",
				Function: wireFunction{Name: tc.Name, Arguments: tc.Arguments}})
		}
		wr.Messages = append(wr.Messages, wm)
	}
	for _, t := range req.Tools {
		var wt wireTool
		wt.Type = "function"
		wt.Function.Name = t.Name
		wt.Function.Description = t.Description
		wt.Function.Parameters = t.Parameters
		if len(wt.Function.Parameters) == 0 {
			wt.Function.Parameters = json.RawMessage(`{"type":"object","properties":{}}`)
		}
		wr.Tools = append(wr.Tools, wt)
	}
	body, err := json.Marshal(wr)
	if err != nil {
		return Response{}, err
	}
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, o.BaseURL+"/chat/completions", bytes.NewReader(body))
	if err != nil {
		return Response{}, err
	}
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Authorization", "Bearer "+o.APIKey)
	httpReq.Header.Set("api-key", o.APIKey) // Azure OpenAI reads this one
	resp, err := o.HTTP.Do(httpReq)
	if err != nil {
		if ctx.Err() != nil {
			return Response{}, err
		}
		return Response{}, &retryableError{err: err}
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		return Response{}, &retryableError{err: err}
	}
	if resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= 500 {
		re := &retryableError{err: fmt.Errorf("llm: status %d: %s", resp.StatusCode, truncate(string(raw), 300))}
		if s, err := strconv.Atoi(resp.Header.Get("Retry-After")); err == nil && s > 0 {
			re.after = time.Duration(s) * time.Second
		}
		return Response{}, re
	}
	var w wireResponse
	if err := json.Unmarshal(raw, &w); err != nil {
		return Response{}, fmt.Errorf("llm: status %d, unreadable body: %s", resp.StatusCode, truncate(string(raw), 300))
	}
	if resp.StatusCode != http.StatusOK || w.Error != nil {
		msg := truncate(string(raw), 300)
		if w.Error != nil {
			msg = w.Error.Message
		}
		return Response{}, fmt.Errorf("llm: status %d: %s", resp.StatusCode, msg)
	}
	if len(w.Choices) == 0 {
		return Response{}, fmt.Errorf("llm: no choices in response")
	}
	out := Response{Usage: w.Usage}
	if w.Choices[0].Message.Content != nil {
		out.Content = *w.Choices[0].Message.Content
	}
	for _, tc := range w.Choices[0].Message.ToolCalls {
		out.ToolCalls = append(out.ToolCalls, ToolCall{ID: tc.ID, Name: tc.Function.Name, Arguments: tc.Function.Arguments})
	}
	return out, nil
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "..."
}

// Stats is what a Recorder has seen so far.
type Stats struct {
	Calls            int           `json:"calls"`
	PromptTokens     int           `json:"prompt_tokens"`
	CompletionTokens int           `json:"completion_tokens"`
	Duration         time.Duration `json:"duration_ns"`
	Errors           int           `json:"errors"`
}

// Recorder wraps a Client, counts calls and tokens, and writes every request
// and response to a directory, one JSON file per call.
type Recorder struct {
	inner Client
	dir   string

	mu    sync.Mutex
	n     int
	stats Stats
}

// NewRecorder wraps inner. If dir is empty nothing is written, only counted.
func NewRecorder(inner Client, dir string) *Recorder {
	return &Recorder{inner: inner, dir: dir}
}

// Stats returns the totals so far.
func (r *Recorder) Stats() Stats {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.stats
}

type record struct {
	At         time.Time `json:"at"`
	DurationMs int64     `json:"duration_ms"`
	Request    Request   `json:"request"`
	Response   *Response `json:"response,omitempty"`
	Error      string    `json:"error,omitempty"`
}

// Chat implements Client.
func (r *Recorder) Chat(ctx context.Context, req Request) (Response, error) {
	start := time.Now()
	resp, err := r.inner.Chat(ctx, req)
	d := time.Since(start)

	r.mu.Lock()
	r.n++
	n := r.n
	r.stats.Calls++
	r.stats.Duration += d
	r.stats.PromptTokens += resp.Usage.PromptTokens
	r.stats.CompletionTokens += resp.Usage.CompletionTokens
	if err != nil {
		r.stats.Errors++
	}
	r.mu.Unlock()

	if r.dir != "" {
		rec := record{At: start, DurationMs: d.Milliseconds(), Request: req}
		if err != nil {
			rec.Error = err.Error()
		} else {
			resp := resp
			rec.Response = &resp
		}
		if b, merr := json.MarshalIndent(rec, "", "  "); merr == nil {
			_ = os.MkdirAll(r.dir, 0o755)
			_ = os.WriteFile(filepath.Join(r.dir, fmt.Sprintf("%03d.json", n)), b, 0o644)
		}
	}
	return resp, err
}
