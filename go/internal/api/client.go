// Package api is the rental system. Three resources: customers, units,
// bookings. Reads filter when asked and never enforce rules. Writes enforce
// every rule and return an *Error with a reason code.
//
// Do not edit this package. Build on top of it.
package api

import "context"

// Client is the rental API. The in-memory implementation in memory.go is what
// the harness uses; there is no HTTP.
type Client interface {
	// IdentifyCustomer matches the caller's phone number and the name they gave.
	// OK is false when either does not match. There is no sign-up on the phone;
	// an unknown caller is a matter for a human.
	IdentifyCustomer(ctx context.Context, phone, name string) (IdentifyResult, error)
	// GetCustomer returns one customer record. 404 if unknown.
	GetCustomer(ctx context.Context, id string) (Customer, error)

	// ListUnits returns the catalogue.
	ListUnits(ctx context.Context) ([]Unit, error)
	// Availability returns free date ranges per unit, with the turnaround day
	// already removed. The filters are optional; without them every unit
	// comes back and the model is left to pick. See AvailabilityQuery.
	Availability(ctx context.Context, q AvailabilityQuery) ([]UnitAvailability, error)

	// ListBookings returns bookings matching the query. It returns other
	// customers' bookings too when the query does not name a customer.
	ListBookings(ctx context.Context, q BookingQuery) ([]Booking, error)
	// CreateBooking books a unit. It needs everything: people, whether the
	// site has power, and what the collecting car can tow. *Error with 422
	// and a reason code when a rule is violated, 409 when the dates are
	// taken, 400 when the request is malformed. See Error.
	CreateBooking(ctx context.Context, req BookingRequest) (Booking, error)
	// UpdateBooking changes or cancels a booking. Version must match the stored
	// one, otherwise 422 VERSION_MISMATCH.
	UpdateBooking(ctx context.Context, id string, version int, patch BookingPatch) (Booking, error)

	// Log returns every call made so far. The harness reads it.
	Log() []Call
}

// Call is one API call as the harness sees it.
type Call struct {
	Method   string `json:"method"`             // "customers.get", "bookings.create", ...
	Status   int    `json:"status"`             // 200, 404, 409, 422, 500
	Code     string `json:"code,omitempty"`     // reason code when Status is not 200
	Detail   string `json:"detail,omitempty"`   // human-readable reason when Status is not 200
	Args     any    `json:"args,omitempty"`     // what was passed in
	Result   any    `json:"result,omitempty"`   // what came back on success
	Injected bool   `json:"injected,omitempty"` // the failure came from a Fault, not from the data
}

// Fault injects a failure into the nth call of a method. The harness uses it
// to simulate dates taken between read and write.
type Fault struct {
	On     string `json:"on" yaml:"on"`                         // "bookings.create"
	Nth    int    `json:"nth" yaml:"nth"`                       // 1 = the first call
	Status int    `json:"status" yaml:"status"`                 // 409 or 500
	Code   string `json:"code,omitempty" yaml:"code,omitempty"` // defaults from Status
}

// Method names as they appear in Call.Method.
const (
	MethodCustomersIdentify = "customers.identify"
	MethodCustomersGet      = "customers.get"
	MethodUnitsList         = "units.list"
	MethodUnitsAvailability = "units.availability"
	MethodBookingsList      = "bookings.list"
	MethodBookingsCreate    = "bookings.create"
	MethodBookingsUpdate    = "bookings.update"
)
