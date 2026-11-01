import { readFile } from "node:fs/promises";

import {
  METHOD_BOOKINGS_CREATE,
  METHOD_BOOKINGS_LIST,
  METHOD_BOOKINGS_UPDATE,
  METHOD_CUSTOMERS_GET,
  METHOD_CUSTOMERS_IDENTIFY,
  METHOD_UNITS_AVAILABILITY,
  METHOD_UNITS_LIST,
  type Call,
  type Client,
  type Fault,
} from "./client.ts";
import {
  ApiError,
  CODE_BAD_DATES,
  CODE_BAD_PEOPLE,
  CODE_BAD_STATUS,
  CODE_CAPACITY_EXCEEDED,
  CODE_CUSTOMER_UNKNOWN,
  CODE_MIN_DAYS_WEEKEND,
  CODE_NOT_FOUND,
  CODE_OVERLAPS_EXISTING,
  CODE_POWER_REQUIRED,
  CODE_TOW_WEIGHT_EXCEEDED,
  CODE_TURNAROUND_CONFLICT,
  CODE_UNAVAILABLE,
  CODE_UNIT_UNKNOWN,
  CODE_VERSION_MISMATCH,
  STATUS_BOOKED,
  STATUS_CANCELLED,
  type AvailabilityQuery,
  type Blocked,
  type Booking,
  type BookingPatch,
  type BookingQuery,
  type BookingRequest,
  type Customer,
  type DateRange,
  type IdentifyResult,
  type Unit,
  type UnitAvailability,
} from "./types.ts";

/** Seed is the database contents a fixture starts from. */
export interface Seed {
  units: Unit[];
  customers: Customer[];
  bookings: Booking[];
  blocked?: Blocked[];
}

/** loadSeed reads a seed JSON file. */
export async function loadSeed(path: string): Promise<Seed> {
  const raw = await readFile(path, "utf8");
  try {
    return JSON.parse(raw) as Seed;
  } catch (err) {
    throw new Error(`${path}: ${(err as Error).message}`, { cause: err });
  }
}

/** Memory is the in-memory implementation of Client. */
export class Memory implements Client {
  // Maps keep insertion order, which is the order the seed listed them in.
  private readonly units = new Map<string, Unit>();
  private readonly customers = new Map<string, Customer>();
  private readonly store = new Map<string, Booking>();
  private readonly blocked: Blocked[];

  private readonly faults: Fault[];
  private readonly counts = new Map<string, number>();
  private readonly calls: Call[] = [];
  private nextId = 100;

  /** Builds a fresh database from a seed. Faults are optional. */
  constructor(seed: Seed, faults: Fault[] = []) {
    for (const u of seed.units) this.units.set(u.id, u);
    for (const c of seed.customers) this.customers.set(c.id, c);
    for (const b of seed.bookings) this.store.set(b.id, b);
    this.blocked = [...(seed.blocked ?? [])];
    this.faults = faults;
  }

  /**
   * bookings returns every booking, seed and new, in insertion order. The
   * harness reads it after a run. It is not part of Client and not logged.
   */
  bookings(): Booking[] {
    return [...this.store.values()];
  }

  log(): Call[] {
    return [...this.calls];
  }

  /**
   * call runs one API method: it applies an injected fault if one is due,
   * runs fn, and records the call with its result or error either way.
   */
  private call<T>(method: string, args: unknown, fn: () => T): T {
    const entry: Call = { method, status: 200 };
    if (args !== undefined) entry.args = args;
    try {
      const injected = this.fault(method);
      if (injected) {
        entry.injected = true;
        throw injected;
      }
      const result = fn();
      entry.result = result;
      return result;
    } catch (err) {
      if (err instanceof ApiError) {
        entry.status = err.status;
        entry.code = err.code;
        entry.detail = err.detail;
      } else {
        entry.status = 500;
        entry.code = "INTERNAL";
        entry.detail = err instanceof Error ? err.message : String(err);
      }
      throw err;
    } finally {
      this.calls.push(entry);
    }
  }

  /** fault returns the injected error for this call of method, if any. */
  private fault(method: string): ApiError | undefined {
    const n = (this.counts.get(method) ?? 0) + 1;
    this.counts.set(method, n);
    const f = this.faults.find((f) => f.on === method && f.nth === n);
    if (!f) return undefined;
    return new ApiError(f.status, f.code || defaultFaultCode(f.status), "injected fault");
  }

  async identifyCustomer(phone: string, name: string): Promise<IdentifyResult> {
    return this.call(METHOD_CUSTOMERS_IDENTIFY, { phone, name }, () => {
      const want = digits(phone);
      const given = nameTokens(name);
      for (const c of this.customers.values()) {
        if (digits(c.phone) !== want) continue;
        if (namesMatch(nameTokens(c.name), given)) return { ok: true, customer_id: c.id };
      }
      return { ok: false };
    });
  }

  async getCustomer(id: string): Promise<Customer> {
    return this.call(METHOD_CUSTOMERS_GET, id, () => {
      const c = this.customers.get(id);
      if (!c) throw new ApiError(404, CODE_NOT_FOUND, `no customer ${id}`);
      return c;
    });
  }

  async listUnits(): Promise<Unit[]> {
    return this.call(METHOD_UNITS_LIST, undefined, () => [...this.units.values()]);
  }

  async availability(q: AvailabilityQuery): Promise<UnitAvailability[]> {
    return this.call(METHOD_UNITS_AVAILABILITY, q, () => {
      const [from, to] = parseRange(q.from, q.to);
      const out: UnitAvailability[] = [];
      for (const u of this.units.values()) {
        if (q.people !== undefined && u.capacity < q.people) continue;
        if (q.max_tow_kg !== undefined && u.gross_weight_kg > q.max_tow_kg) continue;
        if (q.power_available === false && u.needs_power) continue;
        out.push({ unit_id: u.id, free: freeRanges(from, to, this.busyDays(u.id)) });
      }
      return out;
    });
  }

  /** busyDays is every date a unit cannot be collected on: bookings, their turnaround days, and blocks. */
  private busyDays(unitId: string): Set<string> {
    const busy = new Set<string>();
    for (const b of this.store.values()) {
      if (b.unit_id !== unitId || b.status !== STATUS_BOOKED) continue;
      const range = tryParseRange(b.start, b.end);
      if (!range) continue;
      // the turnaround day on either side is busy too
      for (const d of eachDay(addDays(range[0], -1), addDays(range[1], 1))) busy.add(fmtDate(d));
    }
    for (const bl of this.blocked) {
      if (bl.unit_id !== unitId) continue;
      const range = tryParseRange(bl.start, bl.end);
      if (!range) continue;
      for (const d of eachDay(range[0], range[1])) busy.add(fmtDate(d));
    }
    return busy;
  }

  async listBookings(q: BookingQuery): Promise<Booking[]> {
    return this.call(METHOD_BOOKINGS_LIST, q, () => {
      const from = q.from ? parseDate(q.from) : undefined;
      const to = q.to ? parseDate(q.to) : undefined;
      return this.bookings().filter((b) => matchesQuery(b, q, from, to));
    });
  }

  async createBooking(req: BookingRequest): Promise<Booking> {
    return this.call(METHOD_BOOKINGS_CREATE, req, () => {
      req = bookingRequest(req);
      this.validate(req, "");
      this.nextId++;
      const b: Booking = {
        ...req,
        id: `bk_${this.nextId}`,
        version: 1,
        status: STATUS_BOOKED,
        total: this.total(req),
      };
      this.store.set(b.id, b);
      return b;
    });
  }

  async updateBooking(id: string, version: number, patch: BookingPatch): Promise<Booking> {
    return this.call(METHOD_BOOKINGS_UPDATE, { id, version, patch }, () => {
      const cur = this.store.get(id);
      if (!cur) throw new ApiError(404, CODE_NOT_FOUND, `no booking ${id}`);
      if (cur.version !== version) {
        throw new ApiError(409, CODE_VERSION_MISMATCH, `booking ${id} is at version ${cur.version}, got ${version}`);
      }
      if (patch.status !== undefined) {
        if (patch.status !== STATUS_CANCELLED) {
          throw new ApiError(400, CODE_BAD_STATUS, "status can only be set to cancelled");
        }
        const cancelled: Booking = { ...cur, status: STATUS_CANCELLED, version: cur.version + 1 };
        this.store.set(id, cancelled);
        return cancelled;
      }
      const next: Booking = { ...cur };
      if (patch.start !== undefined) next.start = patch.start;
      if (patch.end !== undefined) next.end = patch.end;
      if (patch.people !== undefined) next.people = patch.people;
      if (patch.tow_capacity_kg !== undefined) next.tow_capacity_kg = patch.tow_capacity_kg;
      this.validate(next, id);
      next.total = this.total(next);
      next.version++;
      this.store.set(id, next);
      return next;
    });
  }

  private total(req: BookingRequest): number {
    const u = this.units.get(req.unit_id);
    const range = tryParseRange(req.start, req.end);
    if (!u || !range) return 0;
    return u.price_per_day * daysInclusive(range[0], range[1]);
  }

  /** validate is every business rule. excludeId skips one booking, for updates. */
  private validate(req: BookingRequest, excludeId: string): void {
    if (!this.customers.has(req.customer_id)) {
      throw new ApiError(400, CODE_CUSTOMER_UNKNOWN, `no customer ${req.customer_id}`);
    }
    const u = this.units.get(req.unit_id);
    if (!u) throw new ApiError(400, CODE_UNIT_UNKNOWN, `no unit ${req.unit_id}`);
    const [start, end] = parseRange(req.start, req.end);
    if (!Number.isInteger(req.people) || req.people < 1) {
      throw new ApiError(400, CODE_BAD_PEOPLE, "people must be at least 1");
    }
    checkUnitRules(u, req, start, end);
    this.checkConflicts(u, req, start, end, excludeId);
  }

  /** checkConflicts is the rules that depend on other rows: the caller's own bookings, the unit's, and blocks. */
  private checkConflicts(u: Unit, req: BookingRequest, start: number, end: number, excludeId: string): void {
    for (const b of this.store.values()) {
      if (b.id === excludeId || b.status !== STATUS_BOOKED) continue;
      const range = tryParseRange(b.start, b.end);
      if (!range) continue;
      if (b.customer_id === req.customer_id && overlaps(start, end, range[0], range[1])) {
        throw new ApiError(422, CODE_OVERLAPS_EXISTING, `customer already has ${b.id} from ${b.start} to ${b.end}`);
      }
    }
    for (const b of this.store.values()) {
      if (b.id === excludeId || b.status !== STATUS_BOOKED || b.unit_id !== req.unit_id) continue;
      const range = tryParseRange(b.start, b.end);
      if (!range) continue;
      if (overlaps(start, end, range[0], range[1])) {
        throw new ApiError(409, CODE_UNAVAILABLE, `${u.name} is booked from ${b.start} to ${b.end}`);
      }
      if (adjacent(start, end, range[0], range[1])) {
        throw new ApiError(
          422,
          CODE_TURNAROUND_CONFLICT,
          `${u.name} needs a clear day after the booking ending ${b.end} or before the one starting ${b.start}`,
        );
      }
    }
    for (const bl of this.blocked) {
      if (bl.unit_id !== req.unit_id) continue;
      const range = tryParseRange(bl.start, bl.end);
      if (!range) continue;
      if (overlaps(start, end, range[0], range[1])) {
        throw new ApiError(409, CODE_UNAVAILABLE, `${u.name} is out for ${bl.reason} from ${bl.start} to ${bl.end}`);
      }
    }
  }
}

/** checkUnitRules is the rules that need only the unit and the request. */
function checkUnitRules(u: Unit, req: BookingRequest, start: number, end: number): void {
  if (req.people > u.capacity) {
    throw new ApiError(422, CODE_CAPACITY_EXCEEDED, `${u.name} takes ${u.capacity} people, asked for ${req.people}`);
  }
  if (u.gross_weight_kg > req.tow_capacity_kg) {
    throw new ApiError(
      422,
      CODE_TOW_WEIGHT_EXCEEDED,
      `${u.name} is ${u.gross_weight_kg} kg, stated towing capacity is ${req.tow_capacity_kg} kg`,
    );
  }
  if (u.needs_power && !req.site.power_available) {
    throw new ApiError(422, CODE_POWER_REQUIRED, `${u.name} is electric and the site has no mains power`);
  }
  if (touchesWeekend(start, end) && daysInclusive(start, end) < 2) {
    throw new ApiError(422, CODE_MIN_DAYS_WEEKEND, "rentals that touch a Saturday or Sunday are two days minimum");
  }
}

function matchesQuery(b: Booking, q: BookingQuery, from: number | undefined, to: number | undefined): boolean {
  if (q.customer_id && b.customer_id !== q.customer_id) return false;
  if (q.unit_id && b.unit_id !== q.unit_id) return false;
  if (from === undefined && to === undefined) return true;
  const range = tryParseRange(b.start, b.end);
  if (!range) return false;
  if (from !== undefined && range[1] < from) return false;
  return to === undefined || range[0] <= to;
}

function defaultFaultCode(status: number): string {
  switch (status) {
    case 409:
      return CODE_UNAVAILABLE;
    case 500:
      return "INTERNAL";
    default:
      return "FAULT";
  }
}

/**
 * bookingRequest keeps only the fields the API knows and fills missing ones
 * the way a typed decoder would: empty string, zero, false. Tool arguments
 * arrive as loose JSON, and what gets stored must not depend on what else
 * the model put in there.
 */
function bookingRequest(req: BookingRequest): BookingRequest {
  return {
    customer_id: req.customer_id ?? "",
    unit_id: req.unit_id ?? "",
    start: req.start ?? "",
    end: req.end ?? "",
    people: req.people ?? 0,
    site: { power_available: req.site?.power_available ?? false },
    tow_capacity_kg: req.tow_capacity_kg ?? 0,
  };
}

// dates: a calendar day is its UTC midnight timestamp in milliseconds, so
// days compare with < and > and a day apart is exactly DAY_MS

const DAY_MS = 24 * 60 * 60 * 1000;

function parseDate(s: string): number {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  const d = m ? Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])) : Number.NaN;
  if (Number.isNaN(d) || fmtDate(d) !== s) {
    throw new ApiError(400, CODE_BAD_DATES, `bad date "${s}", want YYYY-MM-DD`);
  }
  return d;
}

function parseRange(from: string, to: string): [number, number] {
  const f = parseDate(from);
  const t = parseDate(to);
  if (t < f) throw new ApiError(400, CODE_BAD_DATES, "end is before start");
  return [f, t];
}

/** tryParseRange is parseRange for stored rows, where a bad date is skipped rather than reported. */
function tryParseRange(from: string, to: string): [number, number] | undefined {
  try {
    return parseRange(from, to);
  } catch {
    return undefined;
  }
}

function fmtDate(d: number): string {
  return new Date(d).toISOString().slice(0, 10);
}

function addDays(d: number, n: number): number {
  return d + n * DAY_MS;
}

function* eachDay(from: number, to: number): Generator<number> {
  for (let d = from; d <= to; d = addDays(d, 1)) yield d;
}

function daysInclusive(s: number, e: number): number {
  return Math.round((e - s) / DAY_MS) + 1;
}

/** freeRanges turns the days between from and to that are not busy into inclusive ranges. */
function freeRanges(from: number, to: number, busy: Set<string>): DateRange[] {
  const free: DateRange[] = [];
  let cur: DateRange | undefined;
  for (const d of eachDay(from, to)) {
    const ds = fmtDate(d);
    if (busy.has(ds)) {
      if (cur) free.push(cur);
      cur = undefined;
    } else if (cur) {
      cur.end = ds;
    } else {
      cur = { start: ds, end: ds };
    }
  }
  if (cur) free.push(cur);
  return free;
}

function overlaps(aS: number, aE: number, bS: number, bE: number): boolean {
  return aE >= bS && bE >= aS;
}

/** adjacent is true when the ranges do not overlap but no clear day separates them. */
function adjacent(aS: number, aE: number, bS: number, bE: number): boolean {
  return aS === addDays(bE, 1) || bS === addDays(aE, 1);
}

function touchesWeekend(s: number, e: number): boolean {
  for (const d of eachDay(s, e)) {
    const wd = new Date(d).getUTCDay();
    if (wd === 0 || wd === 6) return true;
  }
  return false;
}

// names and phones

function digits(s: string): string {
  return s.replace(/\P{Nd}/gu, "");
}

function nameTokens(s: string): string[] {
  return (s.toLowerCase().match(/\p{L}+/gu) ?? []).sort();
}

/**
 * namesMatch accepts the stored name exactly, or every given token present in
 * the stored name when at least two distinct tokens were given. "Tomas.
 * Petrauskas." from a transcript matches "Tomas Petrauskas"; "Tomas" alone
 * does not, and neither does "Tomas Tomas".
 */
function namesMatch(stored: string[], given: string[]): boolean {
  if (given.length === 0) return false;
  if (stored.join(" ") === given.join(" ")) return true;
  const distinct = new Set(given);
  if (distinct.size < 2) return false;
  const have = new Set(stored);
  return [...distinct].every((t) => have.has(t));
}
