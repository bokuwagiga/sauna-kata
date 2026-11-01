/** Unit is one mobile sauna. The rules live in these fields. */
export interface Unit {
  /** "unit_mazoji" */
  id: string;
  /** "Mažoji" */
  name: string;
  /** people */
  capacity: number;
  /** what the car must tow */
  gross_weight_kg: number;
  heater: "wood" | "electric";
  /** electric units need mains at the site */
  needs_power: boolean;
  price_per_day: number;
}

/**
 * Customer is who is calling, once identified. Nothing here says what their
 * car can tow. Ask.
 */
export interface Customer {
  id: string;
  name: string;
  phone: string;
  address: string;
  /** nothing here says what their car can tow. ask. */
  past_bookings: number;
}

/** IdentifyResult is the outcome of matching a phone number and a spoken name. */
export interface IdentifyResult {
  ok: boolean;
  customer_id?: string;
}

/**
 * AvailabilityQuery asks which units are free between from and to, inclusive.
 * The filters are optional. Without them every unit comes back.
 */
export interface AvailabilityQuery {
  /** "2026-10-01" */
  from: string;
  /** inclusive */
  to: string;
  /** keep units with capacity >= people */
  people?: number;
  /** keep units with gross_weight_kg <= max_tow_kg */
  max_tow_kg?: number;
  /** false: drop units that need power */
  power_available?: boolean;
}

/** DateRange is an inclusive range of calendar dates. */
export interface DateRange {
  start: string;
  end: string;
}

/**
 * UnitAvailability lists the free ranges of one unit. The turnaround day on
 * either side of an existing booking is already removed.
 */
export interface UnitAvailability {
  unit_id: string;
  free: DateRange[];
}

/** BookingQuery filters bookings. Every field is optional. */
export interface BookingQuery {
  customer_id?: string;
  unit_id?: string;
  from?: string;
  to?: string;
}

/** Site describes where the sauna is going. */
export interface Site {
  power_available: boolean;
}

/** BookingRequest is everything the API needs to book a unit. */
export interface BookingRequest {
  customer_id: string;
  unit_id: string;
  /** "2026-10-03" */
  start: string;
  /** "2026-10-04", inclusive */
  end: string;
  people: number;
  site: Site;
  /** braked towing capacity of the car collecting it */
  tow_capacity_kg: number;
}

/** Booking statuses. */
export const STATUS_BOOKED = "booked";
export const STATUS_CANCELLED = "cancelled";
export type BookingStatus = typeof STATUS_BOOKED | typeof STATUS_CANCELLED;

/** Booking is a stored booking. */
export interface Booking extends BookingRequest {
  id: string;
  version: number;
  status: BookingStatus;
  total: number;
}

/** BookingPatch changes a booking. Missing fields are left alone. */
export interface BookingPatch {
  start?: string;
  end?: string;
  people?: number;
  tow_capacity_kg?: number;
  /** "cancelled" */
  status?: string;
}

/** Blocked marks days a unit is out of service. */
export interface Blocked {
  unit_id: string;
  start: string;
  end: string;
  reason: string;
}

/** Reason codes. */
export const CODE_UNAVAILABLE = "UNAVAILABLE"; // 409: the unit is booked or out of service on those dates
export const CODE_VERSION_MISMATCH = "VERSION_MISMATCH"; // 409: stale version on update
export const CODE_NOT_FOUND = "NOT_FOUND"; // 404: no such customer or booking

export const CODE_CUSTOMER_UNKNOWN = "CUSTOMER_UNKNOWN"; // 400
export const CODE_UNIT_UNKNOWN = "UNIT_UNKNOWN"; // 400
export const CODE_BAD_DATES = "BAD_DATES"; // 400: not YYYY-MM-DD, or end before start
export const CODE_BAD_PEOPLE = "BAD_PEOPLE"; // 400: people < 1
export const CODE_BAD_STATUS = "BAD_STATUS"; // 400: status can only be set to cancelled

export const CODE_CAPACITY_EXCEEDED = "CAPACITY_EXCEEDED"; // 422: people above the unit's capacity
export const CODE_TOW_WEIGHT_EXCEEDED = "TOW_WEIGHT_EXCEEDED"; // 422: gross_weight_kg above tow_capacity_kg
export const CODE_POWER_REQUIRED = "POWER_REQUIRED"; // 422: unit needs_power and the site has no mains
export const CODE_TURNAROUND_CONFLICT = "TURNAROUND_CONFLICT"; // 422: fewer than one clear day between this and another booking of the unit
export const CODE_MIN_DAYS_WEEKEND = "MIN_DAYS_WEEKEND"; // 422: touches Saturday or Sunday and is shorter than two days
export const CODE_OVERLAPS_EXISTING = "OVERLAPS_EXISTING"; // 422: the customer already has a booking in the range

/**
 * ApiError is what the API throws instead of returning a result.
 *
 *     400  malformed request: bad date, unknown id, people < 1. Your bug.
 *     404  no such record.
 *     409  conflict: the dates are taken, or the version is stale.
 *     422  rule violation: capacity, weight, power, turnaround, weekend minimum,
 *          overlap with the caller's own booking. The receptionist offered
 *          something the rules do not allow.
 */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly detail: string;

  constructor(status: number, code: string, detail: string) {
    super(`${code}: ${detail}`);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.detail = detail;
  }
}
