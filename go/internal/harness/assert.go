package harness

import (
	"fmt"
	"strings"

	"pirtis/internal/api"
	"pirtis/internal/transcript"
)

// World is everything the harness knows after a run.
type World struct {
	Effects    []transcript.Effect // from the last turn
	Calls      []api.Call          // every API call across all turns
	Before     []api.Booking       // bookings when the run started
	After      []api.Booking       // bookings when it ended
	Identified map[string]bool     // customer ids identified in the transcript before the run
}

// Evaluate checks the fixture's expectations and the invariants against the
// world and returns one line per failure. Empty means the run passed.
//
// Invariants, checked for every fixture:
//
//   - No API call returned 4xx or 5xx, unless the failure was injected. A
//     422 means the receptionist offered something the rules do not allow; a
//     400 means the request itself was malformed.
//   - Customer data is only touched for an identified caller. customers.get,
//     bookings.create, bookings.update and a bookings.list naming a customer
//     all require that customer to have been identified, in the transcript
//     or by a successful customers.identify earlier in this run.
func Evaluate(e Expect, w World) []string {
	var fails []string
	add := func(format string, args ...any) { fails = append(fails, fmt.Sprintf(format, args...)) }

	// action
	types := make([]string, 0, len(w.Effects))
	for _, ef := range w.Effects {
		types = append(types, ef.Type())
	}
	want := map[string]string{"say": "speech", "wait": "silence", "transfer": "transfer", "end_call": "hangup"}[e.Action]
	if !contains(types, want) {
		add("action: wanted %s, got effects %v", e.Action, types)
	}
	if e.Action == "wait" && contains(types, "speech") {
		add("action: wanted wait, but there was speech: %q", firstSpeech(w.Effects))
	}

	// bookings diff
	changes := diff(w.Before, w.After)
	matched := make([]bool, len(changes))
	for _, want := range e.Bookings {
		found := false
		for i, c := range changes {
			if !matched[i] && want.matches(c) {
				matched[i] = true
				found = true
				break
			}
		}
		if !found {
			add("bookings: expected %s, got %s", want.String(), describeChanges(changes))
		}
	}
	for i, c := range changes {
		if !matched[i] {
			add("bookings: unexpected %s", c.String())
		}
	}

	// invariants
	identified := map[string]bool{}
	for id := range w.Identified {
		identified[id] = true
	}
	byID := map[string]api.Booking{}
	for _, b := range w.After {
		byID[b.ID] = b
	}
	for _, c := range w.Calls {
		if c.Status >= 400 && !c.Injected {
			add("api: %s returned %d %s: %s (%s)", c.Method, c.Status, c.Code, c.Detail, statusMeaning(c.Status))
		}
		switch c.Method {
		case api.MethodCustomersIdentify:
			if r, ok := c.Result.(api.IdentifyResult); ok && r.OK {
				identified[r.CustomerID] = true
			}
		case api.MethodCustomersGet:
			if id := fmt.Sprint(c.Args); !identified[id] {
				add("api: customers.get(%s) before that caller was identified", id)
			}
		case api.MethodBookingsCreate:
			if req, ok := c.Args.(api.BookingRequest); ok && !identified[req.CustomerID] {
				add("api: bookings.create for %s before that caller was identified", req.CustomerID)
			}
		case api.MethodBookingsUpdate:
			if args, ok := c.Args.(map[string]any); ok {
				id := fmt.Sprint(args["id"])
				if b, ok := byID[id]; ok && !identified[b.CustomerID] {
					add("api: bookings.update(%s) for %s before that caller was identified", id, b.CustomerID)
				}
			}
		case api.MethodBookingsList:
			if q, ok := c.Args.(api.BookingQuery); ok && q.CustomerID != "" && !identified[q.CustomerID] {
				add("api: bookings.list for %s before that caller was identified", q.CustomerID)
			}
		}
	}

	return fails
}

func statusMeaning(status int) string {
	switch {
	case status == 400:
		return "malformed request"
	case status == 404:
		return "no such record"
	case status == 409:
		return "conflict: dates taken or stale version"
	case status == 422:
		return "rule violation: offered something the rules do not allow"
	default:
		return "server error"
	}
}

// change is one created or changed booking.
type change struct {
	Kind string // "created" | "changed"
	B    api.Booking
}

func (c change) String() string {
	return fmt.Sprintf("%s %s", c.Kind, describe(c.B))
}

func describe(b api.Booking) string {
	return fmt.Sprintf("%s(%s %s %s..%s x%d %s v%d)", b.ID, b.CustomerID, b.UnitID, b.Start, b.End, b.People, b.Status, b.Version)
}

func describeChanges(cs []change) string {
	if len(cs) == 0 {
		return "no changes"
	}
	parts := make([]string, 0, len(cs))
	for _, c := range cs {
		parts = append(parts, c.String())
	}
	return strings.Join(parts, ", ")
}

func diff(before, after []api.Booking) []change {
	prev := map[string]api.Booking{}
	for _, b := range before {
		prev[b.ID] = b
	}
	var out []change
	for _, b := range after {
		p, ok := prev[b.ID]
		switch {
		case !ok:
			out = append(out, change{Kind: "created", B: b})
		case p != b:
			out = append(out, change{Kind: "changed", B: b})
		}
	}
	return out
}

func (e BookingExpect) matches(c change) bool {
	b := c.B
	if e.ID != "" && (e.ID != b.ID || c.Kind != "changed") {
		return false
	}
	if e.ID == "" && c.Kind != "created" {
		return false
	}
	if e.Customer != "" && e.Customer != b.CustomerID {
		return false
	}
	if e.Unit != "" && e.Unit != b.UnitID {
		return false
	}
	if e.Start != "" && e.Start != b.Start {
		return false
	}
	if e.End != "" && e.End != b.End {
		return false
	}
	if e.People != nil && *e.People != b.People {
		return false
	}
	status := e.Status
	if status == "" {
		status = api.StatusBooked
	}
	return status == b.Status
}

func (e BookingExpect) String() string {
	var parts []string
	if e.ID != "" {
		parts = append(parts, "id="+e.ID)
	}
	if e.Customer != "" {
		parts = append(parts, "customer="+e.Customer)
	}
	if e.Unit != "" {
		parts = append(parts, "unit="+e.Unit)
	}
	if e.Start != "" {
		parts = append(parts, "start="+e.Start)
	}
	if e.End != "" {
		parts = append(parts, "end="+e.End)
	}
	if e.People != nil {
		parts = append(parts, fmt.Sprintf("people=%d", *e.People))
	}
	if e.Status != "" {
		parts = append(parts, "status="+e.Status)
	}
	kind := "created"
	if e.ID != "" {
		kind = "changed"
	}
	return kind + "{" + strings.Join(parts, " ") + "}"
}

func firstSpeech(effects []transcript.Effect) string {
	for _, ef := range effects {
		if ef.Type() == "speech" {
			return fmt.Sprint(ef["text"])
		}
	}
	return ""
}

func contains(set []string, s string) bool {
	for _, x := range set {
		if x == s {
			return true
		}
	}
	return false
}
