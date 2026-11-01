// The harness module loads fixtures, seeds the API, calls the agent with a
// transcript, runs the handlers and checks the world afterwards.

import { readdir, readFile } from "node:fs/promises";
import path from "node:path";

import { parse as parseYaml } from "yaml";

import type { Fault } from "../api/client.ts";
import { STATUS_BOOKED, type Booking } from "../api/types.ts";
import type { Transcript } from "../transcript/types.ts";

/**
 * Fixture is one test case: the call so far, what the world must look like
 * after the agent's turn, and optionally what a judge should check in the
 * reply. One YAML file each, one turn each: decide is called once with the
 * transcript and the checks run on what it did.
 */
export interface Fixture {
  id: string;
  /** rows added to the base seed */
  setup?: Setup;
  /** make the nth call of a method fail */
  faults?: Fault[];
  expect: Expect;
  /**
   * judge is a list of statements about the reply, each true or false, for
   * fixtures where the right answer is a sentence and not a booking. The
   * same pinned model reads the call and the reply at temperature zero and
   * marks each one with a one-sentence reason; a failed line prints as
   * "judge: <statement>: <reason>". Optional.
   */
  judge?: string[];
  transcript: Transcript;

  /** where the fixture file lives; set by loadFixtures, not by the YAML */
  dir: string;
}

/** Setup adds rows to the base seed before the run. */
export interface Setup {
  bookings?: SetupBooking[];
}

/** SetupBooking is a booking that already exists when the turn starts. */
export interface SetupBooking {
  id: string;
  customer: string;
  unit: string;
  start: string;
  end: string;
  people: number;
  power?: boolean;
  tow?: number;
  version?: number;
  total?: number;
}

/** setupBooking converts a SetupBooking into a Booking. */
export function setupBooking(s: SetupBooking): Booking {
  return {
    customer_id: s.customer,
    unit_id: s.unit,
    start: s.start,
    end: s.end,
    people: s.people,
    site: { power_available: s.power ?? false },
    tow_capacity_kg: s.tow ?? 0,
    id: s.id,
    version: s.version || 1,
    status: STATUS_BOOKED,
    total: s.total ?? 0,
  };
}

export type ExpectAction = "say" | "wait" | "transfer" | "end_call";

const EXPECT_ACTIONS: readonly string[] = ["say", "wait", "transfer", "end_call"];

/**
 * Expect is what must be true after the last turn. Everything not listed
 * here is an invariant the harness always checks; see evaluate.
 */
export interface Expect {
  /**
   * action is the effect type the last turn must produce: say, wait,
   * transfer or end_call. wait also means no speech.
   */
  action: ExpectAction;
  /**
   * bookings is what changed in the bookings table, as a diff against the
   * seed. Empty means nothing changed. Listed fields are compared, the rest
   * are ignored. id names an existing booking that must have changed.
   */
  bookings?: BookingExpect[];
}

/** BookingExpect is one expected created or changed booking. */
export interface BookingExpect {
  id?: string;
  customer?: string;
  unit?: string;
  start?: string;
  end?: string;
  people?: number;
  status?: string;
}

/** loadFixtures reads every *.yaml in dir, sorted by file name. */
export async function loadFixtures(dir: string): Promise<Fixture[]> {
  const names = (await readdir(dir)).filter((n) => n.endsWith(".yaml")).sort();
  const out: Fixture[] = [];
  for (const name of names) {
    const p = path.join(dir, name);
    let f: Fixture;
    try {
      f = parseYaml(await readFile(p, "utf8")) as Fixture;
    } catch (err) {
      throw new Error(`${p}: ${(err as Error).message}`, { cause: err });
    }
    if (!f?.id) throw new Error(`${p}: missing id`);
    if (!f.transcript?.items?.length || !f.transcript.today) {
      throw new Error(`${p}: transcript needs today and items`);
    }
    if (!EXPECT_ACTIONS.includes(f.expect?.action)) {
      throw new Error(`${p}: expect.action must be say, wait, transfer or end_call`);
    }
    f.dir = dir;
    out.push(f);
  }
  return out;
}

/** seedPath is the base seed every fixture starts from. */
export function seedPath(f: Fixture): string {
  return path.join(f.dir, "seeds", "base.json");
}
