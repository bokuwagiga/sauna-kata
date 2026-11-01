import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";

import { METHOD_BOOKINGS_CREATE } from "./client.ts";
import { Memory, loadSeed, type Seed } from "./memory.ts";
import {
  ApiError,
  CODE_BAD_DATES,
  CODE_BAD_STATUS,
  CODE_CAPACITY_EXCEEDED,
  CODE_CUSTOMER_UNKNOWN,
  CODE_MIN_DAYS_WEEKEND,
  CODE_OVERLAPS_EXISTING,
  CODE_POWER_REQUIRED,
  CODE_TOW_WEIGHT_EXCEEDED,
  CODE_TURNAROUND_CONFLICT,
  CODE_UNAVAILABLE,
  CODE_UNIT_UNKNOWN,
  CODE_VERSION_MISMATCH,
  STATUS_BOOKED,
  STATUS_CANCELLED,
  type BookingRequest,
} from "./types.ts";

const SEED_PATH = path.join(import.meta.dirname, "../../../fixtures/seeds/base.json");

function seedForTest(): Promise<Seed> {
  return loadSeed(SEED_PATH);
}

function req(
  cust: string,
  unit: string,
  start: string,
  end: string,
  people: number,
  tow: number,
  power: boolean,
): BookingRequest {
  return {
    customer_id: cust,
    unit_id: unit,
    start,
    end,
    people,
    site: { power_available: power },
    tow_capacity_kg: tow,
  };
}

async function wantCode(p: Promise<unknown>, status: number, code: string): Promise<void> {
  await assert.rejects(p, (err: unknown) => {
    assert.ok(err instanceof ApiError, `want ApiError ${status} ${code}, got ${err}`);
    assert.equal(`${err.status} ${err.code}`, `${status} ${code}`, err.detail);
    return true;
  });
}

test("identify", async () => {
  const m = new Memory(await seedForTest());
  const cases: [string, string, boolean][] = [
    ["+37061234567", "Tomas Petrauskas", true],
    ["+37061234567", "Tomas. Petrauskas.", true],
    ["+370 612 34567", "petrauskas tomas", true],
    ["+37061234567", "Tomas", false],
    ["+37061234567", "Tomas Tomas", false],
    ["+37061234567", "Jonas Jonaitis", false],
    ["+37000000000", "Tomas Petrauskas", false],
    ["+37061234567", "", false],
  ];
  for (const [phone, name, ok] of cases) {
    const r = await m.identifyCustomer(phone, name);
    assert.equal(r.ok, ok, `identify(${JSON.stringify(phone)}, ${JSON.stringify(name)})`);
  }
});

test("rules", async (t) => {
  const cases: [string, BookingRequest, number, string][] = [
    ["ok weekday", req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, false), 0, ""],
    ["ok weekend two days", req("cus_12", "unit_mazoji", "2026-10-10", "2026-10-11", 4, 1500, false), 0, ""],
    ["capacity", req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 5, 1500, false), 422, CODE_CAPACITY_EXCEEDED],
    ["tow weight", req("cus_12", "unit_didzioji", "2026-10-10", "2026-10-11", 6, 1500, false), 422, CODE_TOW_WEIGHT_EXCEEDED],
    ["power", req("cus_12", "unit_vidutine", "2026-10-10", "2026-10-11", 6, 1500, false), 422, CODE_POWER_REQUIRED],
    ["weekend one day", req("cus_12", "unit_mazoji", "2026-10-10", "2026-10-10", 4, 1500, false), 422, CODE_MIN_DAYS_WEEKEND],
    ["weekday one day ok", req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-06", 4, 1500, false), 0, ""],
    ["turnaround after", req("cus_12", "unit_vidutine", "2026-10-03", "2026-10-04", 6, 1500, true), 422, CODE_TURNAROUND_CONFLICT],
    ["turnaround before", req("cus_12", "unit_didzioji", "2026-10-04", "2026-10-05", 8, 2500, true), 422, CODE_TURNAROUND_CONFLICT],
    ["clear day ok", req("cus_12", "unit_vidutine", "2026-10-04", "2026-10-05", 6, 1500, true), 0, ""],
    ["unit taken", req("cus_12", "unit_didzioji", "2026-10-06", "2026-10-07", 8, 2500, true), 409, CODE_UNAVAILABLE],
    ["unit blocked", req("cus_12", "unit_statine", "2026-10-08", "2026-10-09", 4, 1500, true), 409, CODE_UNAVAILABLE],
    ["overlaps own", req("cus_07", "unit_didzioji", "2026-10-17", "2026-10-18", 8, 2500, true), 422, CODE_OVERLAPS_EXISTING],
    ["unknown customer", req("cus_99", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, false), 400, CODE_CUSTOMER_UNKNOWN],
    ["unknown unit", req("cus_12", "unit_x", "2026-10-06", "2026-10-07", 4, 1500, false), 400, CODE_UNIT_UNKNOWN],
    ["bad dates", req("cus_12", "unit_mazoji", "2026-10-07", "2026-10-06", 4, 1500, false), 400, CODE_BAD_DATES],
    ["bad date format", req("cus_12", "unit_mazoji", "6 October", "2026-10-07", 4, 1500, false), 400, CODE_BAD_DATES],
  ];
  for (const [name, r, status, code] of cases) {
    await t.test(name, async () => {
      const m = new Memory(await seedForTest());
      if (status === 0) {
        const b = await m.createBooking(r);
        assert.equal(b.status, STATUS_BOOKED);
        assert.equal(b.version, 1);
        assert.notEqual(b.total, 0);
        return;
      }
      await wantCode(m.createBooking(r), status, code);
    });
  }
});

test("availability: turnaround and filters", async () => {
  const m = new Memory(await seedForTest());

  let out = await m.availability({ from: "2026-10-01", to: "2026-10-11" });
  const byUnit = new Map(out.map((u) => [u.unit_id, u.free]));
  // vidutine booked Sep 30 - Oct 2, so Oct 3 is turnaround; free from Oct 4
  assert.deepEqual(byUnit.get("unit_vidutine"), [{ start: "2026-10-04", end: "2026-10-11" }]);
  // didzioji booked Oct 6-7: free Oct 1-4 and Oct 9-11
  assert.deepEqual(byUnit.get("unit_didzioji"), [
    { start: "2026-10-01", end: "2026-10-04" },
    { start: "2026-10-09", end: "2026-10-11" },
  ]);
  // statine blocked Oct 9
  assert.deepEqual(byUnit.get("unit_statine"), [
    { start: "2026-10-01", end: "2026-10-08" },
    { start: "2026-10-10", end: "2026-10-11" },
  ]);

  out = await m.availability({ from: "2026-10-10", to: "2026-10-11", people: 6, power_available: false });
  assert.deepEqual(
    out.map((u) => u.unit_id),
    ["unit_didzioji"],
    "six people, no power: want only didzioji",
  );

  out = await m.availability({ from: "2026-10-10", to: "2026-10-11", people: 6, max_tow_kg: 1500 });
  assert.deepEqual(
    out.map((u) => u.unit_id),
    ["unit_vidutine"],
    "six people, 1500 kg: want only vidutine",
  );
});

test("update and version", async () => {
  const m = new Memory(await seedForTest());
  const b = await m.createBooking(req("cus_12", "unit_mazoji", "2026-10-03", "2026-10-04", 4, 1500, false));

  await wantCode(m.updateBooking(b.id, 7, { start: "2026-10-10", end: "2026-10-11" }), 409, CODE_VERSION_MISMATCH);

  const b2 = await m.updateBooking(b.id, b.version, { start: "2026-10-10", end: "2026-10-11" });
  assert.equal(b2.version, 2);
  assert.equal(b2.start, "2026-10-10");
  assert.equal(b2.total, 240);

  await wantCode(m.updateBooking(b.id, b2.version, { status: "booked" }), 400, CODE_BAD_STATUS);

  const b3 = await m.updateBooking(b.id, b2.version, { status: STATUS_CANCELLED });
  assert.equal(b3.status, STATUS_CANCELLED);
  assert.equal(b3.version, 3);

  // cancelled bookings free the unit
  await m.createBooking(req("cus_07", "unit_mazoji", "2026-10-10", "2026-10-11", 4, 2000, false));
});

test("faults and log", async () => {
  const m = new Memory(await seedForTest(), [{ on: METHOD_BOOKINGS_CREATE, nth: 1, status: 409 }]);
  const r = req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, false);
  await wantCode(m.createBooking(r), 409, CODE_UNAVAILABLE);
  await m.createBooking(r);

  const log = m.log();
  assert.equal(log.length, 2);
  assert.equal(log[0]!.status, 409);
  assert.equal(log[0]!.injected, true);
  assert.equal(log[0]!.method, METHOD_BOOKINGS_CREATE);
  assert.equal(log[1]!.status, 200);
  assert.equal(m.bookings().length, 4, "want 4 bookings (3 seed + 1)");
});

test("list bookings filters", async () => {
  const m = new Memory(await seedForTest());
  let out = await m.listBookings({ unit_id: "unit_didzioji", from: "2026-10-06", to: "2026-10-07" });
  assert.equal(out.length, 1);
  assert.equal(out[0]!.customer_id, "cus_31");

  out = await m.listBookings({ customer_id: "cus_12" });
  assert.equal(out.length, 0, "cus_12 has no bookings in the seed");
});
