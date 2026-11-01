// Package transcript holds the shapes the harness feeds the agent and the
// shapes the agent returns. Field names follow what voice stacks already use:
// role and item type from the OpenAI Realtime API, is_final and speech_final
// from Deepgram, interrupted from LiveKit, prosody from Hume, and
// generation_config on the output from Cartesia.
package transcript

import (
	"encoding/json"
	"fmt"
	"os"
)

// Transcript is the call so far, as of Now.
//
// Reading it:
//
//   - A user item with IsFinal false is text still changing; IsFinal true
//     and SpeechFinal false is stable text from someone still talking;
//     SpeechFinal true means the speaker paused.
//   - A large gap between Now and the last item's End, with SpeechFinal
//     true, is silence.
//   - A user item whose Start falls inside an assistant item's range is a
//     barge-in; that assistant item carries Interrupted true.
//   - Two user items in a row is a caller who paused and went on.
//   - function_call and function_call_output items are API calls made
//     earlier in the call, named by API method ("customers.identify",
//     "bookings.create"), with the request as Arguments and the response,
//     or {"error", "detail"}, as Output. Whatever tools you expose to the
//     model, the transcript records what reached the API.
//
// Callers are people. They hedge, restart, correct themselves, talk to
// someone in the room, and change the plan halfway.
type Transcript struct {
	CallID string  `json:"call_id" yaml:"call_id"`
	Today  string  `json:"today" yaml:"today"` // "2026-09-28", fixed per fixture so date words resolve
	Now    float64 `json:"now" yaml:"now"`     // seconds since the call started
	Caller Caller  `json:"caller" yaml:"caller"`
	Items  []Item  `json:"items" yaml:"items"`
}

// Caller is what the phone system knows before anyone speaks.
type Caller struct {
	Phone string `json:"phone" yaml:"phone"`
}

// Item is one conversation item. Type decides which fields are set.
//
//	message               Role, Start, End, Content, and either Interrupted
//	                      (assistant) or IsFinal, SpeechFinal, Confidence,
//	                      Prosody (user)
//	function_call         CallID, Name, Arguments, Start
//	function_call_output  CallID, Output, End
//
// Function calls are API calls made earlier in the call, named by API method
// ("customers.identify", "bookings.create"), with the request as Arguments
// and the response, or {"error", "detail"}, as Output.
type Item struct {
	Type string `json:"type" yaml:"type"`

	Role        string   `json:"role,omitempty" yaml:"role,omitempty"` // "user" | "assistant"
	Start       float64  `json:"start" yaml:"start"`
	End         float64  `json:"end" yaml:"end"`
	Content     string   `json:"content,omitempty" yaml:"content,omitempty"`
	Interrupted bool     `json:"interrupted,omitempty" yaml:"interrupted,omitempty"`   // assistant: cut off by a barge-in
	IsFinal     bool     `json:"is_final,omitempty" yaml:"is_final,omitempty"`         // user: transcript text is stable
	SpeechFinal bool     `json:"speech_final,omitempty" yaml:"speech_final,omitempty"` // user: the speaker paused
	Confidence  float64  `json:"confidence,omitempty" yaml:"confidence,omitempty"`
	Prosody     *Prosody `json:"prosody,omitempty" yaml:"prosody,omitempty"`

	CallID    string `json:"call_id,omitempty" yaml:"call_id,omitempty"`
	Name      string `json:"name,omitempty" yaml:"name,omitempty"`
	Arguments string `json:"arguments,omitempty" yaml:"arguments,omitempty"` // JSON text
	Output    string `json:"output,omitempty" yaml:"output,omitempty"`       // JSON text
}

// Prosody is a rough read of how the line was said, not what was said.
type Prosody struct {
	Emotions []Emotion `json:"emotions" yaml:"emotions"`
}

// Emotion is one named dimension and its score in [0, 1].
type Emotion struct {
	Name  string  `json:"name" yaml:"name"`
	Score float64 `json:"score" yaml:"score"`
}

// Action is what the receptionist does next. It is a map with a "type" key so
// new action types and new fields need no schema change. The harness runs each
// action through the handler registered for its type; an action with no
// handler fails the fixture. Say, Wait, Transfer and EndCall below build the
// four the shipped handlers know; build others by hand.
type Action map[string]any

// Effect is what a handler produced when it carried out an action. The
// fixtures assert on effects.
type Effect map[string]any

// Type returns the action's "type" key, or "" if missing.
func (a Action) Type() string {
	s, _ := a["type"].(string)
	return s
}

// Type returns the effect's "type" key, or "" if missing.
func (e Effect) Type() string {
	s, _ := e["type"].(string)
	return s
}

// Say speaks text. Emotion and speed are hints to the TTS; speed is 0.6 to 1.5.
func Say(text, emotion string, speed float64) Action {
	return Action{
		"type": "say",
		"text": text,
		"generation_config": map[string]any{
			"emotion": emotion,
			"speed":   speed,
		},
	}
}

// Wait says nothing for a while and lets the caller finish or think.
func Wait(seconds float64) Action {
	return Action{"type": "wait", "seconds": seconds}
}

// Transfer hands the call to a human queue. The queue name is yours to choose.
func Transfer(to, reason string) Action {
	return Action{"type": "transfer", "to": to, "reason": reason}
}

// EndCall hangs up.
func EndCall(reason string) Action {
	return Action{"type": "end_call", "reason": reason}
}

// LoadJSON reads a transcript from a JSON file, for your own tests. The
// fixtures carry their transcripts inline in YAML with the same keys.
func LoadJSON(path string) (Transcript, error) {
	var t Transcript
	b, err := os.ReadFile(path)
	if err != nil {
		return t, err
	}
	if err := json.Unmarshal(b, &t); err != nil {
		return t, fmt.Errorf("%s: %w", path, err)
	}
	return t, nil
}
