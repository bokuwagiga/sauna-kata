//! The rental system. Three resources: customers, units, bookings. Reads
//! filter when asked and never enforce rules. Writes enforce every rule and
//! return an [`Error`] with a reason code.
//!
//! Do not edit this module. Build on top of it.

mod memory;
mod types;

pub use memory::{load_seed, Memory, Seed};
pub use types::*;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Client is the rental API. The in-memory implementation in memory.rs is
/// what the harness uses; there is no HTTP.
pub trait Client {
    /// Matches the caller's phone number and the name they gave. `ok` is false
    /// when either does not match. There is no sign-up on the phone; an
    /// unknown caller is a matter for a human.
    fn identify_customer(&self, phone: &str, name: &str) -> Result<IdentifyResult, Error>;
    /// Returns one customer record. 404 if unknown.
    fn get_customer(&self, id: &str) -> Result<Customer, Error>;

    /// Returns the catalogue.
    fn list_units(&self) -> Result<Vec<Unit>, Error>;
    /// Returns free date ranges per unit, with the turnaround day already
    /// removed. The filters are optional; without them every unit comes back
    /// and the model is left to pick. See [`AvailabilityQuery`].
    fn availability(&self, q: &AvailabilityQuery) -> Result<Vec<UnitAvailability>, Error>;

    /// Returns bookings matching the query. It returns other customers'
    /// bookings too when the query does not name a customer.
    fn list_bookings(&self, q: &BookingQuery) -> Result<Vec<Booking>, Error>;
    /// Books a unit. It needs everything: people, whether the site has power,
    /// and what the collecting car can tow. [`Error`] with 422 and a reason
    /// code when a rule is violated, 409 when the dates are taken, 400 when
    /// the request is malformed. See [`Error`].
    fn create_booking(&self, req: &BookingRequest) -> Result<Booking, Error>;
    /// Changes or cancels a booking. `version` must match the stored one,
    /// otherwise 409 VERSION_MISMATCH.
    fn update_booking(&self, id: &str, version: i32, patch: &BookingPatch) -> Result<Booking, Error>;

    /// Returns every call made so far. The harness reads it.
    fn log(&self) -> Vec<Call>;
}

/// Call is one API call as the harness sees it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Call {
    /// "customers.get", "bookings.create", ...
    pub method: String,
    /// 200, 404, 409, 422, 500
    pub status: u16,
    /// reason code when status is not 200
    #[serde(skip_serializing_if = "String::is_empty")]
    pub code: String,
    /// human-readable reason when status is not 200
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// what was passed in
    #[serde(skip_serializing_if = "Value::is_null")]
    pub args: Value,
    /// what came back on success
    #[serde(skip_serializing_if = "Value::is_null")]
    pub result: Value,
    /// the failure came from a Fault, not from the data
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub injected: bool,
}

/// Fault injects a failure into the nth call of a method. The harness uses it
/// to simulate dates taken between read and write.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fault {
    /// "bookings.create"
    pub on: String,
    /// 1 = the first call
    pub nth: i32,
    /// 409 or 500
    pub status: u16,
    /// defaults from status
    #[serde(skip_serializing_if = "String::is_empty")]
    pub code: String,
}

// Method names as they appear in Call.method.
pub const METHOD_CUSTOMERS_IDENTIFY: &str = "customers.identify";
pub const METHOD_CUSTOMERS_GET: &str = "customers.get";
pub const METHOD_UNITS_LIST: &str = "units.list";
pub const METHOD_UNITS_AVAILABILITY: &str = "units.availability";
pub const METHOD_BOOKINGS_LIST: &str = "bookings.list";
pub const METHOD_BOOKINGS_CREATE: &str = "bookings.create";
pub const METHOD_BOOKINGS_UPDATE: &str = "bookings.update";
