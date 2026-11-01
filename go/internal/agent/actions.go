package agent

import (
	"context"

	"pirtis/internal/api"
	"pirtis/internal/transcript"
)

// HandlerContext is what a handler can reach while carrying out an action.
type HandlerContext struct {
	API        api.Client
	Transcript transcript.Transcript
}

// Handler carries out one action and reports what happened as an Effect.
type Handler func(ctx context.Context, a transcript.Action, hc HandlerContext) (transcript.Effect, error)

// Handlers maps an action type to the code that carries it out. The harness
// runs every action Decide returns through the handler for its type and
// asserts on the effects. An action with no handler fails the fixture.
//
// These four are the starting set. Add your own, change these, or wrap them.
var Handlers = map[string]Handler{
	"say": func(_ context.Context, a transcript.Action, _ HandlerContext) (transcript.Effect, error) {
		return transcript.Effect{
			"type":              "speech",
			"text":              a["text"],
			"generation_config": a["generation_config"],
		}, nil
	},
	"wait": func(_ context.Context, a transcript.Action, _ HandlerContext) (transcript.Effect, error) {
		return transcript.Effect{"type": "silence", "seconds": a["seconds"]}, nil
	},
	"transfer": func(_ context.Context, a transcript.Action, _ HandlerContext) (transcript.Effect, error) {
		return transcript.Effect{"type": "transfer", "to": a["to"], "reason": a["reason"]}, nil
	},
	"end_call": func(_ context.Context, a transcript.Action, _ HandlerContext) (transcript.Effect, error) {
		return transcript.Effect{"type": "hangup", "reason": a["reason"]}, nil
	},
}
