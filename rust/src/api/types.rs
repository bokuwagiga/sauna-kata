use serde::{Deserialize, Serialize};

/// Unit is one mobile sauna. The rules live in these fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Unit {
    /// "unit_mazoji"
    pub id: String,
    /// "Mažoji"
    pub name: String,
    /// people
    pub capacity: i32,
    /// what the car must tow
    pub gross_weight_kg: i32,
    /// "wood" | "electric"
    pub heater: String,
    /// electric units need mains at the site
    pub needs_power: bool,
    pub price_per_day: i32,
}

/// Customer is who is calling, once identified. Nothing here says what their
/// car can tow. Ask.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Customer {
    pub id: String,
    pub name: String,
    pub phone: String,
    pub address: String,
    /// nothing here says what their car can tow. ask.
    pub past_bookings: i32,
}

/// IdentifyResult is the outcome of matching a phone number and a spoken name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdentifyResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub customer_id: String,
}

/// AvailabilityQuery asks which units are free between `from` and `to`,
/// inclusive. The filters are optional. Without them every unit comes back.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AvailabilityQuery {
    /// "2026-10-01"
    pub from: String,
    /// inclusive
    pub to: String,
    /// keep units with capacity >= people
    #[serde(skip_serializing_if = "Option::is_none")]
    pub people: Option<i32>,
    /// keep units with gross_weight_kg <= max_tow_kg
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tow_kg: Option<i32>,
    /// false: drop units that need power
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_available: Option<bool>,
}

/// DateRange is an inclusive range of calendar dates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DateRange {
    pub start: String,
    pub end: String,
}

/// UnitAvailability lists the free ranges of one unit. The turnaround day on
/// either side of an existing booking is already removed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UnitAvailability {
    pub unit_id: String,
    pub free: Vec<DateRange>,
}

/// BookingQuery filters bookings. Every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BookingQuery {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub customer_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unit_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
}

/// Site describes where the sauna is going.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Site {
    pub power_available: bool,
}

/// BookingRequest is everything the API needs to book a unit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BookingRequest {
    pub customer_id: String,
    pub unit_id: String,
    /// "2026-10-03"
    pub start: String,
    /// "2026-10-04", inclusive
    pub end: String,
    pub people: i32,
    pub site: Site,
    /// braked towing capacity of the car collecting it
    pub tow_capacity_kg: i32,
}

/// Booking is a stored booking.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Booking {
    #[serde(flatten)]
    pub request: BookingRequest,
    pub id: String,
    pub version: i32,
    /// "booked" | "cancelled"
    pub status: String,
    pub total: i32,
}

/// BookingPatch changes a booking. `None` fields are left alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BookingPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub people: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tow_capacity_kg: Option<i32>,
    /// "cancelled"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Blocked marks days a unit is out of service.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Blocked {
    pub unit_id: String,
    pub start: String,
    pub end: String,
    pub reason: String,
}

// Booking statuses.
pub const STATUS_BOOKED: &str = "booked";
pub const STATUS_CANCELLED: &str = "cancelled";

/// Error is what the API returns instead of a result.
///
/// ```text
/// 400  malformed request: bad date, unknown id, people < 1. Your bug.
/// 404  no such record.
/// 409  conflict: the dates are taken, or the version is stale.
/// 422  rule violation: capacity, weight, power, turnaround, weekend minimum,
///      overlap with the caller's own booking. The receptionist offered
///      something the rules do not allow.
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct Error {
    pub status: u16,
    pub code: String,
    pub detail: String,
}

impl Error {
    pub fn new(status: u16, code: &str, detail: impl Into<String>) -> Error {
        Error { status, code: code.to_string(), detail: detail.into() }
    }
}

// Reason codes.
/// 409: the unit is booked or out of service on those dates
pub const CODE_UNAVAILABLE: &str = "UNAVAILABLE";
/// 409: stale version on update
pub const CODE_VERSION_MISMATCH: &str = "VERSION_MISMATCH";
/// 404: no such customer or booking
pub const CODE_NOT_FOUND: &str = "NOT_FOUND";

/// 400
pub const CODE_CUSTOMER_UNKNOWN: &str = "CUSTOMER_UNKNOWN";
/// 400
pub const CODE_UNIT_UNKNOWN: &str = "UNIT_UNKNOWN";
/// 400: not YYYY-MM-DD, or end before start
pub const CODE_BAD_DATES: &str = "BAD_DATES";
/// 400: people < 1
pub const CODE_BAD_PEOPLE: &str = "BAD_PEOPLE";
/// 400: status can only be set to cancelled
pub const CODE_BAD_STATUS: &str = "BAD_STATUS";

/// 422: people above the unit's capacity
pub const CODE_CAPACITY_EXCEEDED: &str = "CAPACITY_EXCEEDED";
/// 422: gross_weight_kg above tow_capacity_kg
pub const CODE_TOW_WEIGHT_EXCEEDED: &str = "TOW_WEIGHT_EXCEEDED";
/// 422: unit needs_power and the site has no mains
pub const CODE_POWER_REQUIRED: &str = "POWER_REQUIRED";
/// 422: fewer than one clear day between this and another booking of the unit
pub const CODE_TURNAROUND_CONFLICT: &str = "TURNAROUND_CONFLICT";
/// 422: touches Saturday or Sunday and is shorter than two days
pub const CODE_MIN_DAYS_WEEKEND: &str = "MIN_DAYS_WEEKEND";
/// 422: the customer already has a booking in the range
pub const CODE_OVERLAPS_EXISTING: &str = "OVERLAPS_EXISTING";
