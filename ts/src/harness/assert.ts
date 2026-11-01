import { isDeepStrictEqual } from "node:util";

import {
  METHOD_BOOKINGS_CREATE,
  METHOD_BOOKINGS_LIST,
  METHOD_BOOKINGS_UPDATE,
  METHOD_CUSTOMERS_GET,
  METHOD_CUSTOMERS_IDENTIFY,
  type Call,
} from "../api/client.ts";
import { STATUS_BOOKED, type Booking, type BookingQuery, type BookingRequest, type IdentifyResult } from "../api/types.ts";
import type { Effect } from "../transcript/types.ts";
import type { BookingExpect, Expect } from "./fixture.ts";

/** World is everything the harness knows after a run. */
export interface World {
  /** from the last turn */
  effects: Effect[];
  /** every API call across all turns */
  calls: Call[];
  /** bookings when the run started */
  before: Booking[];
  /** bookings when it ended */
  after: Booking[];
  /** customer ids identified in the transcript before the run */
  identified: Set<string>;
}

const WANTED_EFFECT: Record<Expect["action"], string> = {
  say: "speech",
  wait: "silence",
  transfer: "transfer",
  end_call: "hangup",
};

/**
 * evaluate checks the fixture's expectations and the invariants against the
 * world and returns one line per failure. Empty means the run passed.
 *
 * Invariants, checked for every fixture:
 *
 * - No API call returned 4xx or 5xx, unless the failure was injected. A
 *   422 means the receptionist offered something the rules do not allow; a
 *   400 means the request itself was malformed.
 * - Customer data is only touched for an identified caller. customers.get,
 *   bookings.create, bookings.update and a bookings.list naming a customer
 *   all require that customer to have been identified, in the transcript
 *   or by a successful customers.identify earlier in this run.
 */
export function evaluate(e: Expect, w: World): string[] {
  const fails: string[] = [];

  // action
  const types = w.effects.map((ef) => ef.type);
  if (!types.includes(WANTED_EFFECT[e.action])) {
    fails.push(`action: wanted ${e.action}, got effects [${types.join(" ")}]`);
  }
  if (e.action === "wait" && types.includes("speech")) {
    fails.push(`action: wanted wait, but there was speech: ${JSON.stringify(firstSpeech(w.effects))}`);
  }

  // bookings diff
  const changes = diff(w.before, w.after);
  const matched = new Array<boolean>(changes.length).fill(false);
  for (const want of e.bookings ?? []) {
    const i = changes.findIndex((c, i) => !matched[i] && matches(want, c));
    if (i < 0) fails.push(`bookings: expected ${expectString(want)}, got ${describeChanges(changes)}`);
    else matched[i] = true;
  }
  changes.forEach((c, i) => {
    if (!matched[i]) fails.push(`bookings: unexpected ${changeString(c)}`);
  });

  // invariants
  const identified = new Set(w.identified);
  const byId = new Map(w.after.map((b) => [b.id, b]));
  for (const c of w.calls) {
    if (c.status >= 400 && !c.injected) {
      fails.push(`api: ${c.method} returned ${c.status} ${c.code}: ${c.detail} (${statusMeaning(c.status)})`);
    }
    switch (c.method) {
      case METHOD_CUSTOMERS_IDENTIFY: {
        const r = c.result as IdentifyResult | undefined;
        if (r?.ok && r.customer_id) identified.add(r.customer_id);
        break;
      }
      case METHOD_CUSTOMERS_GET: {
        const id = String(c.args);
        if (!identified.has(id)) fails.push(`api: customers.get(${id}) before that caller was identified`);
        break;
      }
      case METHOD_BOOKINGS_CREATE: {
        const req = c.args as BookingRequest;
        if (!identified.has(req.customer_id)) {
          fails.push(`api: bookings.create for ${req.customer_id} before that caller was identified`);
        }
        break;
      }
      case METHOD_BOOKINGS_UPDATE: {
        const id = String((c.args as { id: unknown }).id);
        const b = byId.get(id);
        if (b && !identified.has(b.customer_id)) {
          fails.push(`api: bookings.update(${id}) for ${b.customer_id} before that caller was identified`);
        }
        break;
      }
      case METHOD_BOOKINGS_LIST: {
        const q = c.args as BookingQuery;
        if (q.customer_id && !identified.has(q.customer_id)) {
          fails.push(`api: bookings.list for ${q.customer_id} before that caller was identified`);
        }
        break;
      }
    }
  }

  return fails;
}

function statusMeaning(status: number): string {
  switch (status) {
    case 400:
      return "malformed request";
    case 404:
      return "no such record";
    case 409:
      return "conflict: dates taken or stale version";
    case 422:
      return "rule violation: offered something the rules do not allow";
    default:
      return "server error";
  }
}

/** Change is one created or changed booking. */
interface Change {
  kind: "created" | "changed";
  booking: Booking;
}

function changeString(c: Change): string {
  return `${c.kind} ${describe(c.booking)}`;
}

function describe(b: Booking): string {
  return `${b.id}(${b.customer_id} ${b.unit_id} ${b.start}..${b.end} x${b.people} ${b.status} v${b.version})`;
}

function describeChanges(cs: Change[]): string {
  return cs.length === 0 ? "no changes" : cs.map(changeString).join(", ");
}

function diff(before: Booking[], after: Booking[]): Change[] {
  const prev = new Map(before.map((b) => [b.id, b]));
  const out: Change[] = [];
  for (const b of after) {
    const p = prev.get(b.id);
    if (!p) out.push({ kind: "created", booking: b });
    else if (!isDeepStrictEqual(p, b)) out.push({ kind: "changed", booking: b });
  }
  return out;
}

function matches(e: BookingExpect, c: Change): boolean {
  const b = c.booking;
  if (e.id && (e.id !== b.id || c.kind !== "changed")) return false;
  if (!e.id && c.kind !== "created") return false;
  if (e.customer && e.customer !== b.customer_id) return false;
  if (e.unit && e.unit !== b.unit_id) return false;
  if (e.start && e.start !== b.start) return false;
  if (e.end && e.end !== b.end) return false;
  if (e.people !== undefined && e.people !== b.people) return false;
  return (e.status || STATUS_BOOKED) === b.status;
}

function expectString(e: BookingExpect): string {
  const parts: string[] = [];
  if (e.id) parts.push(`id=${e.id}`);
  if (e.customer) parts.push(`customer=${e.customer}`);
  if (e.unit) parts.push(`unit=${e.unit}`);
  if (e.start) parts.push(`start=${e.start}`);
  if (e.end) parts.push(`end=${e.end}`);
  if (e.people !== undefined) parts.push(`people=${e.people}`);
  if (e.status) parts.push(`status=${e.status}`);
  const kind = e.id ? "changed" : "created";
  return `${kind}{${parts.join(" ")}}`;
}

function firstSpeech(effects: Effect[]): string {
  const speech = effects.find((ef) => ef.type === "speech");
  return speech ? String(speech.text) : "";
}
