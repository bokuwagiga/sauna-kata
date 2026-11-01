// The api module is the rental system. Three resources: customers, units,
// bookings. Reads filter when asked and never enforce rules. Writes enforce
// every rule and throw an ApiError with a reason code.
//
// Do not edit this module. Build on top of it.

import type {
  AvailabilityQuery,
  Booking,
  BookingPatch,
  BookingQuery,
  BookingRequest,
  Customer,
  IdentifyResult,
  Unit,
  UnitAvailability,
} from "./types.ts";

/**
 * Client is the rental API. The in-memory implementation in memory.ts is what
 * the harness uses; there is no HTTP.
 */
export interface Client {
  /**
   * identifyCustomer matches the caller's phone number and the name they gave.
   * ok is false when either does not match. There is no sign-up on the phone;
   * an unknown caller is a matter for a human.
   */
  identifyCustomer(phone: string, name: string): Promise<IdentifyResult>;
  /** getCustomer returns one customer record. 404 if unknown. */
  getCustomer(id: string): Promise<Customer>;

  /** listUnits returns the catalogue. */
  listUnits(): Promise<Unit[]>;
  /**
   * availability returns free date ranges per unit, with the turnaround day
   * already removed. The filters are optional; without them every unit
   * comes back and the model is left to pick. See AvailabilityQuery.
   */
  availability(q: AvailabilityQuery): Promise<UnitAvailability[]>;

  /**
   * listBookings returns bookings matching the query. It returns other
   * customers' bookings too when the query does not name a customer.
   */
  listBookings(q: BookingQuery): Promise<Booking[]>;
  /**
   * createBooking books a unit. It needs everything: people, whether the
   * site has power, and what the collecting car can tow. ApiError with 422
   * and a reason code when a rule is violated, 409 when the dates are
   * taken, 400 when the request is malformed. See ApiError.
   */
  createBooking(req: BookingRequest): Promise<Booking>;
  /**
   * updateBooking changes or cancels a booking. version must match the stored
   * one, otherwise 409 VERSION_MISMATCH.
   */
  updateBooking(id: string, version: number, patch: BookingPatch): Promise<Booking>;

  /** log returns every call made so far. The harness reads it. */
  log(): Call[];
}

/** Call is one API call as the harness sees it. */
export interface Call {
  /** "customers.get", "bookings.create", ... */
  method: string;
  /** 200, 404, 409, 422, 500 */
  status: number;
  /** reason code when status is not 200 */
  code?: string;
  /** human-readable reason when status is not 200 */
  detail?: string;
  /** what was passed in */
  args?: unknown;
  /** what came back on success */
  result?: unknown;
  /** the failure came from a Fault, not from the data */
  injected?: boolean;
}

/**
 * Fault injects a failure into the nth call of a method. The harness uses it
 * to simulate dates taken between read and write.
 */
export interface Fault {
  /** "bookings.create" */
  on: string;
  /** 1 = the first call */
  nth: number;
  /** 409 or 500 */
  status: number;
  /** defaults from status */
  code?: string;
}

/** Method names as they appear in Call.method. */
export const METHOD_CUSTOMERS_IDENTIFY = "customers.identify";
export const METHOD_CUSTOMERS_GET = "customers.get";
export const METHOD_UNITS_LIST = "units.list";
export const METHOD_UNITS_AVAILABILITY = "units.availability";
export const METHOD_BOOKINGS_LIST = "bookings.list";
export const METHOD_BOOKINGS_CREATE = "bookings.create";
export const METHOD_BOOKINGS_UPDATE = "bookings.update";
