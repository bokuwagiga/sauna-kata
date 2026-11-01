package llm

import (
	"context"
	"sync"
)

// Fake answers from a script, then from Default. It is for tests: the
// harness's own, and yours, for driving your loop through a scripted model
// without spending tokens. It does not understand anything.
type Fake struct {
	Script  []Response
	Default Response

	mu sync.Mutex
	i  int
}

// NewFake returns a Fake that replays script in order, then keeps returning
// a fixed line of text with no tool calls.
func NewFake(script ...Response) *Fake {
	return &Fake{
		Script:  script,
		Default: Response{Content: "This is the fake model."},
	}
}

// Chat implements Client.
func (f *Fake) Chat(_ context.Context, _ Request) (Response, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.i < len(f.Script) {
		r := f.Script[f.i]
		f.i++
		return r, nil
	}
	return f.Default, nil
}
