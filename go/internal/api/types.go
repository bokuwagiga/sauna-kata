package api

// Unit is one mobile sauna. The rules live in these fields.
type Unit struct {
	ID            string `json:"id"`              // "unit_mazoji"
	Name          string `json:"name"`            // "Mažoji"
	Capacity      int    `json:"capacity"`        // people
	GrossWeightKg int    `json:"gross_weight_kg"` // what the car must tow
	Heater        string `json:"heater"`          // "wood" | "electric"
	NeedsPower    bool   `json:"needs_power"`     // electric units need mains at the site
	PricePerDay   int    `json:"price_per_day"`
}

// Customer is who is calling, once identified. Nothing here says what their
// car can tow. Ask.
type Customer struct {
	ID           string `json:"id"`
	Name         string `json:"name"`
	Phone        string `json:"phone"`
	Address      string `json:"address"`
	PastBookings int    `json:"past_bookings"` // nothing here says what their car can tow. ask.
}

// IdentifyResult is the outcome of matching a phone number and a spoken name.
type IdentifyResult struct {
	OK         bool   `json:"ok"`
	CustomerID string `json:"customer_id,omitempty"`
}

// AvailabilityQuery asks which units are free between From and To, inclusive.
// The filters are optional. Without them every unit comes back.
type AvailabilityQuery struct {
	From           string `json:"from"`                      // "2026-10-01"
	To             string `json:"to"`                        // inclusive
	People         *int   `json:"people,omitempty"`          // keep units with Capacity >= People
	MaxTowKg       *int   `json:"max_tow_kg,omitempty"`      // keep units with GrossWeightKg <= MaxTowKg
	PowerAvailable *bool  `json:"power_available,omitempty"` // false: drop units that need power
}

// DateRange is an inclusive range of calendar dates.
type DateRange struct {
	Start string `json:"start"`
	End   string `json:"end"`
}

// UnitAvailability lists the free ranges of one unit. The turnaround day on
// either side of an existing booking is already removed.
type UnitAvailability struct {
	UnitID string      `json:"unit_id"`
	Free   []DateRange `json:"free"`
}

// BookingQuery filters bookings. Every field is optional.
type BookingQuery struct {
	CustomerID string `json:"customer_id,omitempty"`
	UnitID     string `json:"unit_id,omitempty"`
	From       string `json:"from,omitempty"`
	To         string `json:"to,omitempty"`
}

// Site describes where the sauna is going.
type Site struct {
	PowerAvailable bool `json:"power_available"`
}

// BookingRequest is everything the API needs to book a unit.
type BookingRequest struct {
	CustomerID    string `json:"customer_id"`
	UnitID        string `json:"unit_id"`
	Start         string `json:"start"` // "2026-10-03"
	End           string `json:"end"`   // "2026-10-04", inclusive
	People        int    `json:"people"`
	Site          Site   `json:"site"`
	TowCapacityKg int    `json:"tow_capacity_kg"` // braked towing capacity of the car collecting it
}

// Booking is a stored booking.
type Booking struct {
	BookingRequest
	ID      string `json:"id"`
	Version int    `json:"version"`
	Status  string `json:"status"` // "booked" | "cancelled"
	Total   int    `json:"total"`
}

// BookingPatch changes a booking. Nil fields are left alone.
type BookingPatch struct {
	Start         *string `json:"start,omitempty"`
	End           *string `json:"end,omitempty"`
	People        *int    `json:"people,omitempty"`
	TowCapacityKg *int    `json:"tow_capacity_kg,omitempty"`
	Status        *string `json:"status,omitempty"` // "cancelled"
}

// Blocked marks days a unit is out of service.
type Blocked struct {
	UnitID string `json:"unit_id"`
	Start  string `json:"start"`
	End    string `json:"end"`
	Reason string `json:"reason"`
}

// Booking statuses.
const (
	StatusBooked    = "booked"
	StatusCancelled = "cancelled"
)

// Error is what the API returns instead of a result.
//
//	400  malformed request: bad date, unknown id, people < 1. Your bug.
//	404  no such record.
//	409  conflict: the dates are taken, or the version is stale.
//	422  rule violation: capacity, weight, power, turnaround, weekend minimum,
//	     overlap with the caller's own booking. The receptionist offered
//	     something the rules do not allow.
type Error struct {
	Status int    `json:"status"`
	Code   string `json:"code"`
	Detail string `json:"detail"`
}

func (e *Error) Error() string { return e.Code + ": " + e.Detail }

// Reason codes.
const (
	CodeUnavailable     = "UNAVAILABLE"      // 409: the unit is booked or out of service on those dates
	CodeVersionMismatch = "VERSION_MISMATCH" // 409: stale version on update
	CodeNotFound        = "NOT_FOUND"        // 404: no such customer or booking

	CodeCustomerUnknown = "CUSTOMER_UNKNOWN" // 400
	CodeUnitUnknown     = "UNIT_UNKNOWN"     // 400
	CodeBadDates        = "BAD_DATES"        // 400: not YYYY-MM-DD, or end before start
	CodeBadPeople       = "BAD_PEOPLE"       // 400: people < 1
	CodeBadStatus       = "BAD_STATUS"       // 400: status can only be set to cancelled

	CodeCapacityExceeded   = "CAPACITY_EXCEEDED"   // 422: People above the unit's Capacity
	CodeTowWeightExceeded  = "TOW_WEIGHT_EXCEEDED" // 422: GrossWeightKg above TowCapacityKg
	CodePowerRequired      = "POWER_REQUIRED"      // 422: unit NeedsPower and the site has no mains
	CodeTurnaroundConflict = "TURNAROUND_CONFLICT" // 422: fewer than one clear day between this and another booking of the unit
	CodeMinDaysWeekend     = "MIN_DAYS_WEEKEND"    // 422: touches Saturday or Sunday and is shorter than two days
	CodeOverlapsExisting   = "OVERLAPS_EXISTING"   // 422: the customer already has a booking in the range
)

// Ptr returns a pointer to v. Handy for the optional query fields.
func Ptr[T any](v T) *T { return &v }
