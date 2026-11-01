package api

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode"
)

// Seed is the database contents a fixture starts from.
type Seed struct {
	Units     []Unit     `json:"units"`
	Customers []Customer `json:"customers"`
	Bookings  []Booking  `json:"bookings"`
	Blocked   []Blocked  `json:"blocked"`
}

// LoadSeed reads a seed JSON file.
func LoadSeed(path string) (Seed, error) {
	var s Seed
	b, err := os.ReadFile(path)
	if err != nil {
		return s, err
	}
	if err := json.Unmarshal(b, &s); err != nil {
		return s, fmt.Errorf("%s: %w", path, err)
	}
	return s, nil
}

// Memory is the in-memory implementation of Client.
type Memory struct {
	mu sync.Mutex

	units     map[string]Unit
	unitOrder []string
	customers map[string]Customer
	custOrder []string
	bookings  map[string]Booking
	bookOrder []string
	blocked   []Blocked

	faults []Fault
	counts map[string]int
	calls  []Call
	nextID int
}

// NewMemory builds a fresh database from a seed. Faults are optional.
func NewMemory(seed Seed, faults []Fault) *Memory {
	m := &Memory{
		units:     map[string]Unit{},
		customers: map[string]Customer{},
		bookings:  map[string]Booking{},
		counts:    map[string]int{},
		faults:    faults,
		nextID:    100,
	}
	for _, u := range seed.Units {
		m.units[u.ID] = u
		m.unitOrder = append(m.unitOrder, u.ID)
	}
	for _, c := range seed.Customers {
		m.customers[c.ID] = c
		m.custOrder = append(m.custOrder, c.ID)
	}
	for _, b := range seed.Bookings {
		m.bookings[b.ID] = b
		m.bookOrder = append(m.bookOrder, b.ID)
	}
	m.blocked = append(m.blocked, seed.Blocked...)
	return m
}

var _ Client = (*Memory)(nil)

// Bookings returns every booking, seed and new, in insertion order. The
// harness reads it after a run. It is not part of Client and not logged.
func (m *Memory) Bookings() []Booking {
	m.mu.Lock()
	defer m.mu.Unlock()
	out := make([]Booking, 0, len(m.bookOrder))
	for _, id := range m.bookOrder {
		out = append(out, m.bookings[id])
	}
	return out
}

// Log implements Client.
func (m *Memory) Log() []Call {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]Call(nil), m.calls...)
}

func (m *Memory) record(method string, args, result any, err error, injected bool) {
	c := Call{Method: method, Status: 200, Args: args, Injected: injected}
	var e *Error
	if errors.As(err, &e) {
		c.Status = e.Status
		c.Code = e.Code
		c.Detail = e.Detail
	} else if err != nil {
		c.Status = 500
		c.Code = "INTERNAL"
		c.Detail = err.Error()
	} else {
		c.Result = result
	}
	m.calls = append(m.calls, c)
}

// fault returns the injected error for this call of method, if any.
func (m *Memory) fault(method string) (error, bool) {
	m.counts[method]++
	n := m.counts[method]
	for _, f := range m.faults {
		if f.On != method || f.Nth != n {
			continue
		}
		code := f.Code
		if code == "" {
			switch f.Status {
			case 409:
				code = CodeUnavailable
			case 500:
				code = "INTERNAL"
			default:
				code = "FAULT"
			}
		}
		return &Error{Status: f.Status, Code: code, Detail: "injected fault"}, true
	}
	return nil, false
}

// IdentifyCustomer implements Client.
func (m *Memory) IdentifyCustomer(_ context.Context, phone, name string) (res IdentifyResult, err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	args := map[string]string{"phone": phone, "name": name}
	injected := false
	defer func() { m.record(MethodCustomersIdentify, args, res, err, injected) }()
	if e, inj := m.fault(MethodCustomersIdentify); e != nil {
		err, injected = e, inj
		return IdentifyResult{}, err
	}
	want := digits(phone)
	given := nameTokens(name)
	for _, id := range m.custOrder {
		c := m.customers[id]
		if digits(c.Phone) != want {
			continue
		}
		if namesMatch(nameTokens(c.Name), given) {
			return IdentifyResult{OK: true, CustomerID: c.ID}, nil
		}
	}
	return IdentifyResult{OK: false}, nil
}

// GetCustomer implements Client.
func (m *Memory) GetCustomer(_ context.Context, id string) (c Customer, err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	injected := false
	defer func() { m.record(MethodCustomersGet, id, c, err, injected) }()
	if e, inj := m.fault(MethodCustomersGet); e != nil {
		err, injected = e, inj
		return Customer{}, err
	}
	c, ok := m.customers[id]
	if !ok {
		return Customer{}, &Error{Status: 404, Code: CodeNotFound, Detail: "no customer " + id}
	}
	return c, nil
}

// ListUnits implements Client.
func (m *Memory) ListUnits(_ context.Context) (out []Unit, err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	injected := false
	defer func() { m.record(MethodUnitsList, nil, out, err, injected) }()
	if e, inj := m.fault(MethodUnitsList); e != nil {
		err, injected = e, inj
		return nil, err
	}
	for _, id := range m.unitOrder {
		out = append(out, m.units[id])
	}
	return out, nil
}

// Availability implements Client.
func (m *Memory) Availability(_ context.Context, q AvailabilityQuery) (out []UnitAvailability, err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	injected := false
	defer func() { m.record(MethodUnitsAvailability, q, out, err, injected) }()
	if e, inj := m.fault(MethodUnitsAvailability); e != nil {
		err, injected = e, inj
		return nil, err
	}
	from, to, err := parseRange(q.From, q.To)
	if err != nil {
		return nil, err
	}
	for _, id := range m.unitOrder {
		u := m.units[id]
		if q.People != nil && u.Capacity < *q.People {
			continue
		}
		if q.MaxTowKg != nil && u.GrossWeightKg > *q.MaxTowKg {
			continue
		}
		if q.PowerAvailable != nil && !*q.PowerAvailable && u.NeedsPower {
			continue
		}
		busy := map[string]bool{}
		for _, bid := range m.bookOrder {
			b := m.bookings[bid]
			if b.UnitID != u.ID || b.Status != StatusBooked {
				continue
			}
			bs, be, perr := parseRange(b.Start, b.End)
			if perr != nil {
				continue
			}
			// the turnaround day on either side is busy too
			for d := bs.AddDate(0, 0, -1); !d.After(be.AddDate(0, 0, 1)); d = d.AddDate(0, 0, 1) {
				busy[fmtDate(d)] = true
			}
		}
		for _, bl := range m.blocked {
			if bl.UnitID != u.ID {
				continue
			}
			bs, be, perr := parseRange(bl.Start, bl.End)
			if perr != nil {
				continue
			}
			for d := bs; !d.After(be); d = d.AddDate(0, 0, 1) {
				busy[fmtDate(d)] = true
			}
		}
		ua := UnitAvailability{UnitID: u.ID, Free: []DateRange{}}
		var cur *DateRange
		for d := from; !d.After(to); d = d.AddDate(0, 0, 1) {
			ds := fmtDate(d)
			if busy[ds] {
				if cur != nil {
					ua.Free = append(ua.Free, *cur)
					cur = nil
				}
				continue
			}
			if cur == nil {
				cur = &DateRange{Start: ds, End: ds}
			} else {
				cur.End = ds
			}
		}
		if cur != nil {
			ua.Free = append(ua.Free, *cur)
		}
		out = append(out, ua)
	}
	return out, nil
}

// ListBookings implements Client.
func (m *Memory) ListBookings(_ context.Context, q BookingQuery) (out []Booking, err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	injected := false
	defer func() { m.record(MethodBookingsList, q, out, err, injected) }()
	if e, inj := m.fault(MethodBookingsList); e != nil {
		err, injected = e, inj
		return nil, err
	}
	var from, to time.Time
	if q.From != "" {
		if from, err = parseDate(q.From); err != nil {
			return nil, err
		}
	}
	if q.To != "" {
		if to, err = parseDate(q.To); err != nil {
			return nil, err
		}
	}
	out = []Booking{}
	for _, id := range m.bookOrder {
		b := m.bookings[id]
		if q.CustomerID != "" && b.CustomerID != q.CustomerID {
			continue
		}
		if q.UnitID != "" && b.UnitID != q.UnitID {
			continue
		}
		if q.From != "" || q.To != "" {
			bs, be, perr := parseRange(b.Start, b.End)
			if perr != nil {
				continue
			}
			if q.From != "" && be.Before(from) {
				continue
			}
			if q.To != "" && bs.After(to) {
				continue
			}
		}
		out = append(out, b)
	}
	return out, nil
}

// CreateBooking implements Client.
func (m *Memory) CreateBooking(_ context.Context, req BookingRequest) (b Booking, err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	injected := false
	defer func() { m.record(MethodBookingsCreate, req, b, err, injected) }()
	if e, inj := m.fault(MethodBookingsCreate); e != nil {
		err, injected = e, inj
		return Booking{}, err
	}
	if err = m.validate(req, ""); err != nil {
		return Booking{}, err
	}
	m.nextID++
	b = Booking{
		BookingRequest: req,
		ID:             fmt.Sprintf("bk_%d", m.nextID),
		Version:        1,
		Status:         StatusBooked,
		Total:          m.total(req),
	}
	m.bookings[b.ID] = b
	m.bookOrder = append(m.bookOrder, b.ID)
	return b, nil
}

// UpdateBooking implements Client.
func (m *Memory) UpdateBooking(_ context.Context, id string, version int, patch BookingPatch) (b Booking, err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	args := map[string]any{"id": id, "version": version, "patch": patch}
	injected := false
	defer func() { m.record(MethodBookingsUpdate, args, b, err, injected) }()
	if e, inj := m.fault(MethodBookingsUpdate); e != nil {
		err, injected = e, inj
		return Booking{}, err
	}
	cur, ok := m.bookings[id]
	if !ok {
		return Booking{}, &Error{Status: 404, Code: CodeNotFound, Detail: "no booking " + id}
	}
	if cur.Version != version {
		return Booking{}, &Error{Status: 409, Code: CodeVersionMismatch,
			Detail: fmt.Sprintf("booking %s is at version %d, got %d", id, cur.Version, version)}
	}
	if patch.Status != nil {
		if *patch.Status != StatusCancelled {
			return Booking{}, &Error{Status: 400, Code: CodeBadStatus, Detail: "status can only be set to cancelled"}
		}
		cur.Status = StatusCancelled
		cur.Version++
		m.bookings[id] = cur
		return cur, nil
	}
	next := cur
	if patch.Start != nil {
		next.Start = *patch.Start
	}
	if patch.End != nil {
		next.End = *patch.End
	}
	if patch.People != nil {
		next.People = *patch.People
	}
	if patch.TowCapacityKg != nil {
		next.TowCapacityKg = *patch.TowCapacityKg
	}
	if err = m.validate(next.BookingRequest, id); err != nil {
		return Booking{}, err
	}
	next.Total = m.total(next.BookingRequest)
	next.Version++
	m.bookings[id] = next
	return next, nil
}

func (m *Memory) total(req BookingRequest) int {
	u := m.units[req.UnitID]
	s, e, err := parseRange(req.Start, req.End)
	if err != nil {
		return 0
	}
	return u.PricePerDay * daysInclusive(s, e)
}

// validate is every business rule. excludeID skips one booking, for updates.
func (m *Memory) validate(req BookingRequest, excludeID string) error {
	if _, ok := m.customers[req.CustomerID]; !ok {
		return &Error{Status: 400, Code: CodeCustomerUnknown, Detail: "no customer " + req.CustomerID}
	}
	u, ok := m.units[req.UnitID]
	if !ok {
		return &Error{Status: 400, Code: CodeUnitUnknown, Detail: "no unit " + req.UnitID}
	}
	start, end, err := parseRange(req.Start, req.End)
	if err != nil {
		return err
	}
	if req.People < 1 {
		return &Error{Status: 400, Code: CodeBadPeople, Detail: "people must be at least 1"}
	}
	if req.People > u.Capacity {
		return &Error{Status: 422, Code: CodeCapacityExceeded,
			Detail: fmt.Sprintf("%s takes %d people, asked for %d", u.Name, u.Capacity, req.People)}
	}
	if u.GrossWeightKg > req.TowCapacityKg {
		return &Error{Status: 422, Code: CodeTowWeightExceeded,
			Detail: fmt.Sprintf("%s is %d kg, stated towing capacity is %d kg", u.Name, u.GrossWeightKg, req.TowCapacityKg)}
	}
	if u.NeedsPower && !req.Site.PowerAvailable {
		return &Error{Status: 422, Code: CodePowerRequired,
			Detail: fmt.Sprintf("%s is electric and the site has no mains power", u.Name)}
	}
	days := daysInclusive(start, end)
	if touchesWeekend(start, end) && days < 2 {
		return &Error{Status: 422, Code: CodeMinDaysWeekend,
			Detail: "rentals that touch a Saturday or Sunday are two days minimum"}
	}
	for _, id := range m.bookOrder {
		b := m.bookings[id]
		if b.ID == excludeID || b.Status != StatusBooked {
			continue
		}
		bs, be, perr := parseRange(b.Start, b.End)
		if perr != nil {
			continue
		}
		if b.CustomerID == req.CustomerID && overlaps(start, end, bs, be) {
			return &Error{Status: 422, Code: CodeOverlapsExisting,
				Detail: fmt.Sprintf("customer already has %s from %s to %s", b.ID, b.Start, b.End)}
		}
	}
	for _, id := range m.bookOrder {
		b := m.bookings[id]
		if b.ID == excludeID || b.Status != StatusBooked || b.UnitID != req.UnitID {
			continue
		}
		bs, be, perr := parseRange(b.Start, b.End)
		if perr != nil {
			continue
		}
		if overlaps(start, end, bs, be) {
			return &Error{Status: 409, Code: CodeUnavailable,
				Detail: fmt.Sprintf("%s is booked from %s to %s", u.Name, b.Start, b.End)}
		}
		if adjacent(start, end, bs, be) {
			return &Error{Status: 422, Code: CodeTurnaroundConflict,
				Detail: fmt.Sprintf("%s needs a clear day after the booking ending %s or before the one starting %s", u.Name, b.End, b.Start)}
		}
	}
	for _, bl := range m.blocked {
		if bl.UnitID != req.UnitID {
			continue
		}
		bs, be, perr := parseRange(bl.Start, bl.End)
		if perr != nil {
			continue
		}
		if overlaps(start, end, bs, be) {
			return &Error{Status: 409, Code: CodeUnavailable,
				Detail: fmt.Sprintf("%s is out for %s from %s to %s", u.Name, bl.Reason, bl.Start, bl.End)}
		}
	}
	return nil
}

// dates

const dateLayout = "2006-01-02"

func parseDate(s string) (time.Time, error) {
	t, err := time.Parse(dateLayout, s)
	if err != nil {
		return time.Time{}, &Error{Status: 400, Code: CodeBadDates, Detail: fmt.Sprintf("bad date %q, want YYYY-MM-DD", s)}
	}
	return t, nil
}

func parseRange(from, to string) (time.Time, time.Time, error) {
	f, err := parseDate(from)
	if err != nil {
		return time.Time{}, time.Time{}, err
	}
	t, err := parseDate(to)
	if err != nil {
		return time.Time{}, time.Time{}, err
	}
	if t.Before(f) {
		return time.Time{}, time.Time{}, &Error{Status: 400, Code: CodeBadDates, Detail: "end is before start"}
	}
	return f, t, nil
}

func fmtDate(t time.Time) string { return t.Format(dateLayout) }

func daysInclusive(s, e time.Time) int { return int(e.Sub(s).Hours()/24) + 1 }

func overlaps(aS, aE, bS, bE time.Time) bool { return !aE.Before(bS) && !bE.Before(aS) }

// adjacent is true when the ranges do not overlap but no clear day separates them.
func adjacent(aS, aE, bS, bE time.Time) bool {
	return aS.Equal(bE.AddDate(0, 0, 1)) || bS.Equal(aE.AddDate(0, 0, 1))
}

func touchesWeekend(s, e time.Time) bool {
	for d := s; !d.After(e); d = d.AddDate(0, 0, 1) {
		if wd := d.Weekday(); wd == time.Saturday || wd == time.Sunday {
			return true
		}
	}
	return false
}

// names and phones

func digits(s string) string {
	var b strings.Builder
	for _, r := range s {
		if unicode.IsDigit(r) {
			b.WriteRune(r)
		}
	}
	return b.String()
}

func nameTokens(s string) []string {
	var out []string
	var cur strings.Builder
	flush := func() {
		if cur.Len() > 0 {
			out = append(out, cur.String())
			cur.Reset()
		}
	}
	for _, r := range strings.ToLower(s) {
		if unicode.IsLetter(r) {
			cur.WriteRune(r)
		} else {
			flush()
		}
	}
	flush()
	sort.Strings(out)
	return out
}

// namesMatch accepts the stored name exactly, or every given token present in
// the stored name when at least two distinct tokens were given. "Tomas.
// Petrauskas." from a transcript matches "Tomas Petrauskas"; "Tomas" alone
// does not, and neither does "Tomas Tomas".
func namesMatch(stored, given []string) bool {
	if len(given) == 0 {
		return false
	}
	if strings.Join(stored, " ") == strings.Join(given, " ") {
		return true
	}
	distinct := map[string]bool{}
	for _, t := range given {
		distinct[t] = true
	}
	if len(distinct) < 2 {
		return false
	}
	have := map[string]bool{}
	for _, t := range stored {
		have[t] = true
	}
	for t := range distinct {
		if !have[t] {
			return false
		}
	}
	return true
}
