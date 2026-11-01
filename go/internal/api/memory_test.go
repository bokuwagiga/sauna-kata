package api

import (
	"context"
	"errors"
	"testing"
)

func seedForTest(t *testing.T) Seed {
	t.Helper()
	s, err := LoadSeed("../../../fixtures/seeds/base.json")
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func req(cust, unit, start, end string, people, tow int, power bool) BookingRequest {
	return BookingRequest{CustomerID: cust, UnitID: unit, Start: start, End: end, People: people,
		Site: Site{PowerAvailable: power}, TowCapacityKg: tow}
}

func wantCode(t *testing.T, err error, status int, code string) {
	t.Helper()
	var e *Error
	if !errors.As(err, &e) {
		t.Fatalf("want *Error %d %s, got %v", status, code, err)
	}
	if e.Status != status || e.Code != code {
		t.Fatalf("want %d %s, got %d %s (%s)", status, code, e.Status, e.Code, e.Detail)
	}
}

func TestIdentify(t *testing.T) {
	m := NewMemory(seedForTest(t), nil)
	ctx := context.Background()
	cases := []struct {
		phone, name string
		ok          bool
	}{
		{"+37061234567", "Tomas Petrauskas", true},
		{"+37061234567", "Tomas. Petrauskas.", true},
		{"+370 612 34567", "petrauskas tomas", true},
		{"+37061234567", "Tomas", false},
		{"+37061234567", "Tomas Tomas", false},
		{"+37061234567", "Jonas Jonaitis", false},
		{"+37000000000", "Tomas Petrauskas", false},
		{"+37061234567", "", false},
	}
	for _, c := range cases {
		r, err := m.IdentifyCustomer(ctx, c.phone, c.name)
		if err != nil {
			t.Fatal(err)
		}
		if r.OK != c.ok {
			t.Errorf("identify(%q, %q) ok=%v want %v", c.phone, c.name, r.OK, c.ok)
		}
	}
}

func TestRules(t *testing.T) {
	ctx := context.Background()
	cases := []struct {
		name   string
		req    BookingRequest
		status int
		code   string
	}{
		{"ok weekday", req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, false), 0, ""},
		{"ok weekend two days", req("cus_12", "unit_mazoji", "2026-10-10", "2026-10-11", 4, 1500, false), 0, ""},
		{"capacity", req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 5, 1500, false), 422, CodeCapacityExceeded},
		{"tow weight", req("cus_12", "unit_didzioji", "2026-10-10", "2026-10-11", 6, 1500, false), 422, CodeTowWeightExceeded},
		{"power", req("cus_12", "unit_vidutine", "2026-10-10", "2026-10-11", 6, 1500, false), 422, CodePowerRequired},
		{"weekend one day", req("cus_12", "unit_mazoji", "2026-10-10", "2026-10-10", 4, 1500, false), 422, CodeMinDaysWeekend},
		{"weekday one day ok", req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-06", 4, 1500, false), 0, ""},
		{"turnaround after", req("cus_12", "unit_vidutine", "2026-10-03", "2026-10-04", 6, 1500, true), 422, CodeTurnaroundConflict},
		{"turnaround before", req("cus_12", "unit_didzioji", "2026-10-04", "2026-10-05", 8, 2500, true), 422, CodeTurnaroundConflict},
		{"clear day ok", req("cus_12", "unit_vidutine", "2026-10-04", "2026-10-05", 6, 1500, true), 0, ""},
		{"unit taken", req("cus_12", "unit_didzioji", "2026-10-06", "2026-10-07", 8, 2500, true), 409, CodeUnavailable},
		{"unit blocked", req("cus_12", "unit_statine", "2026-10-08", "2026-10-09", 4, 1500, true), 409, CodeUnavailable},
		{"overlaps own", req("cus_07", "unit_didzioji", "2026-10-17", "2026-10-18", 8, 2500, true), 422, CodeOverlapsExisting},
		{"unknown customer", req("cus_99", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, false), 400, CodeCustomerUnknown},
		{"unknown unit", req("cus_12", "unit_x", "2026-10-06", "2026-10-07", 4, 1500, false), 400, CodeUnitUnknown},
		{"bad dates", req("cus_12", "unit_mazoji", "2026-10-07", "2026-10-06", 4, 1500, false), 400, CodeBadDates},
		{"bad date format", req("cus_12", "unit_mazoji", "6 October", "2026-10-07", 4, 1500, false), 400, CodeBadDates},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			m := NewMemory(seedForTest(t), nil)
			b, err := m.CreateBooking(ctx, c.req)
			if c.status == 0 {
				if err != nil {
					t.Fatalf("want ok, got %v", err)
				}
				if b.Status != StatusBooked || b.Version != 1 || b.Total == 0 {
					t.Fatalf("bad booking %+v", b)
				}
				return
			}
			wantCode(t, err, c.status, c.code)
		})
	}
}

func TestAvailabilityTurnaroundAndFilters(t *testing.T) {
	m := NewMemory(seedForTest(t), nil)
	ctx := context.Background()

	out, err := m.Availability(ctx, AvailabilityQuery{From: "2026-10-01", To: "2026-10-11"})
	if err != nil {
		t.Fatal(err)
	}
	byUnit := map[string]UnitAvailability{}
	for _, u := range out {
		byUnit[u.UnitID] = u
	}
	// vidutine booked Sep 30 - Oct 2, so Oct 3 is turnaround; free from Oct 4
	if got := byUnit["unit_vidutine"].Free; len(got) != 1 || got[0].Start != "2026-10-04" || got[0].End != "2026-10-11" {
		t.Errorf("vidutine free = %v", got)
	}
	// didzioji booked Oct 6-7: free Oct 1-4 and Oct 9-11
	if got := byUnit["unit_didzioji"].Free; len(got) != 2 || got[0].End != "2026-10-04" || got[1].Start != "2026-10-09" {
		t.Errorf("didzioji free = %v", got)
	}
	// statine blocked Oct 9
	if got := byUnit["unit_statine"].Free; len(got) != 2 || got[0].End != "2026-10-08" || got[1].Start != "2026-10-10" {
		t.Errorf("statine free = %v", got)
	}

	out, err = m.Availability(ctx, AvailabilityQuery{From: "2026-10-10", To: "2026-10-11",
		People: Ptr(6), PowerAvailable: Ptr(false)})
	if err != nil {
		t.Fatal(err)
	}
	if len(out) != 1 || out[0].UnitID != "unit_didzioji" {
		t.Errorf("six people, no power: want only didzioji, got %v", out)
	}

	out, err = m.Availability(ctx, AvailabilityQuery{From: "2026-10-10", To: "2026-10-11",
		People: Ptr(6), MaxTowKg: Ptr(1500)})
	if err != nil {
		t.Fatal(err)
	}
	if len(out) != 1 || out[0].UnitID != "unit_vidutine" {
		t.Errorf("six people, 1500 kg: want only vidutine, got %v", out)
	}
}

func TestUpdateAndVersion(t *testing.T) {
	m := NewMemory(seedForTest(t), nil)
	ctx := context.Background()
	b, err := m.CreateBooking(ctx, req("cus_12", "unit_mazoji", "2026-10-03", "2026-10-04", 4, 1500, false))
	if err != nil {
		t.Fatal(err)
	}
	_, err = m.UpdateBooking(ctx, b.ID, 7, BookingPatch{Start: Ptr("2026-10-10"), End: Ptr("2026-10-11")})
	wantCode(t, err, 409, CodeVersionMismatch)

	b2, err := m.UpdateBooking(ctx, b.ID, b.Version, BookingPatch{Start: Ptr("2026-10-10"), End: Ptr("2026-10-11")})
	if err != nil {
		t.Fatal(err)
	}
	if b2.Version != 2 || b2.Start != "2026-10-10" || b2.Total != 240 {
		t.Fatalf("bad update %+v", b2)
	}

	_, err = m.UpdateBooking(ctx, b.ID, b2.Version, BookingPatch{Status: Ptr("booked")})
	wantCode(t, err, 400, CodeBadStatus)

	b3, err := m.UpdateBooking(ctx, b.ID, b2.Version, BookingPatch{Status: Ptr(StatusCancelled)})
	if err != nil {
		t.Fatal(err)
	}
	if b3.Status != StatusCancelled || b3.Version != 3 {
		t.Fatalf("bad cancel %+v", b3)
	}
	// cancelled bookings free the unit
	if _, err := m.CreateBooking(ctx, req("cus_07", "unit_mazoji", "2026-10-10", "2026-10-11", 4, 2000, false)); err != nil {
		t.Fatalf("after cancel: %v", err)
	}
}

func TestFaultsAndLog(t *testing.T) {
	m := NewMemory(seedForTest(t), []Fault{{On: MethodBookingsCreate, Nth: 1, Status: 409}})
	ctx := context.Background()
	r := req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, false)
	_, err := m.CreateBooking(ctx, r)
	wantCode(t, err, 409, CodeUnavailable)
	if _, err := m.CreateBooking(ctx, r); err != nil {
		t.Fatalf("second create: %v", err)
	}
	log := m.Log()
	if len(log) != 2 || log[0].Status != 409 || log[1].Status != 200 || log[0].Method != MethodBookingsCreate {
		t.Fatalf("log = %+v", log)
	}
	if got := m.Bookings(); len(got) != 4 {
		t.Fatalf("want 4 bookings (3 seed + 1), got %d", len(got))
	}
}

func TestListBookingsFilters(t *testing.T) {
	m := NewMemory(seedForTest(t), nil)
	ctx := context.Background()
	out, err := m.ListBookings(ctx, BookingQuery{UnitID: "unit_didzioji", From: "2026-10-06", To: "2026-10-07"})
	if err != nil {
		t.Fatal(err)
	}
	if len(out) != 1 || out[0].CustomerID != "cus_31" {
		t.Fatalf("want bk_88 for cus_31, got %+v", out)
	}
	out, _ = m.ListBookings(ctx, BookingQuery{CustomerID: "cus_12"})
	if len(out) != 0 {
		t.Fatalf("cus_12 has no bookings in the seed, got %+v", out)
	}
}
