// Package harness loads fixtures, seeds the API, calls the agent with a
// transcript, runs the handlers and checks the world afterwards.
package harness

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"gopkg.in/yaml.v3"

	"pirtis/internal/api"
	"pirtis/internal/transcript"
)

// Fixture is one test case: the call so far, what the world must look like
// after the agent's turn, and optionally what a judge should check in the
// reply. One YAML file each, one turn each: Decide is called once with the
// transcript and the checks run on what it did.
type Fixture struct {
	ID     string      `yaml:"id"`
	Setup  Setup       `yaml:"setup"`  // rows added to the base seed
	Faults []api.Fault `yaml:"faults"` // make the nth call of a method fail
	Expect Expect      `yaml:"expect"`
	// Judge is a list of statements about the reply, each true or false, for
	// fixtures where the right answer is a sentence and not a booking. The
	// same pinned model reads the call and the reply at temperature zero and
	// marks each one with a one-sentence reason; a failed line prints as
	// "judge: <statement>: <reason>". Optional.
	Judge      []string              `yaml:"judge"`
	Transcript transcript.Transcript `yaml:"transcript"`

	Dir string `yaml:"-"` // where the fixture file lives
}

// Setup adds rows to the base seed before the run.
type Setup struct {
	Bookings []SetupBooking `yaml:"bookings"`
}

// SetupBooking is a booking that already exists when the turn starts.
type SetupBooking struct {
	ID       string `yaml:"id"`
	Customer string `yaml:"customer"`
	Unit     string `yaml:"unit"`
	Start    string `yaml:"start"`
	End      string `yaml:"end"`
	People   int    `yaml:"people"`
	Power    bool   `yaml:"power"`
	Tow      int    `yaml:"tow"`
	Version  int    `yaml:"version"`
	Total    int    `yaml:"total"`
}

// Booking converts a SetupBooking into an api.Booking.
func (s SetupBooking) Booking() api.Booking {
	v := s.Version
	if v == 0 {
		v = 1
	}
	return api.Booking{
		BookingRequest: api.BookingRequest{
			CustomerID:    s.Customer,
			UnitID:        s.Unit,
			Start:         s.Start,
			End:           s.End,
			People:        s.People,
			Site:          api.Site{PowerAvailable: s.Power},
			TowCapacityKg: s.Tow,
		},
		ID:      s.ID,
		Version: v,
		Status:  api.StatusBooked,
		Total:   s.Total,
	}
}

// Expect is what must be true after the last turn. Everything not listed
// here is an invariant the harness always checks; see Evaluate.
type Expect struct {
	// Action is the effect type the last turn must produce: say, wait,
	// transfer or end_call. wait also means no speech.
	Action string `yaml:"action"`
	// Bookings is what changed in the bookings table, as a diff against the
	// seed. Empty means nothing changed. Listed fields are compared, the rest
	// are ignored. ID names an existing booking that must have changed.
	Bookings []BookingExpect `yaml:"bookings"`
}

// BookingExpect is one expected created or changed booking.
type BookingExpect struct {
	ID       string `yaml:"id"`
	Customer string `yaml:"customer"`
	Unit     string `yaml:"unit"`
	Start    string `yaml:"start"`
	End      string `yaml:"end"`
	People   *int   `yaml:"people"`
	Status   string `yaml:"status"`
}

// LoadFixtures reads every *.yaml in dir, sorted by file name.
func LoadFixtures(dir string) ([]Fixture, error) {
	paths, err := filepath.Glob(filepath.Join(dir, "*.yaml"))
	if err != nil {
		return nil, err
	}
	sort.Strings(paths)
	var out []Fixture
	for _, p := range paths {
		b, err := os.ReadFile(p)
		if err != nil {
			return nil, err
		}
		var f Fixture
		if err := yaml.Unmarshal(b, &f); err != nil {
			return nil, fmt.Errorf("%s: %w", p, err)
		}
		if f.ID == "" {
			return nil, fmt.Errorf("%s: missing id", p)
		}
		if len(f.Transcript.Items) == 0 || f.Transcript.Today == "" {
			return nil, fmt.Errorf("%s: transcript needs today and items", p)
		}
		switch f.Expect.Action {
		case "say", "wait", "transfer", "end_call":
		default:
			return nil, fmt.Errorf("%s: expect.action must be say, wait, transfer or end_call", p)
		}
		f.Dir = dir
		out = append(out, f)
	}
	return out, nil
}

// SeedPath is the base seed every fixture starts from.
func (f Fixture) SeedPath() string {
	return filepath.Join(f.Dir, "seeds", "base.json")
}
