import assert from "node:assert/strict";
import { stat } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";

import type { Handler } from "../agent/actions.ts";
import { Fake, fake } from "../llm/fake.ts";
import { say, wait, type Action } from "../transcript/types.ts";
import { loadFixtures, seedPath, type Fixture } from "./fixture.ts";
import { Runner, type DecideFunc } from "./run.ts";

// These tests drive the harness with stub agents, never with the shipped
// agent, so a candidate's rewrite of src/agent cannot break them.

const FIXTURES_DIR = path.join(import.meta.dirname, "../../../fixtures");

async function load(id: string): Promise<Fixture> {
  const f = (await loadFixtures(FIXTURES_DIR)).find((f) => f.id === id);
  assert.ok(f, `no fixture ${JSON.stringify(id)}`);
  return f;
}

// handlers is the effect contract the harness checks against: say -> speech,
// wait -> silence, transfer -> transfer, end_call -> hangup.
const handlers: Record<string, Handler> = {
  say: (a) => ({ type: "speech", text: a.text }),
  wait: (a) => ({ type: "silence", seconds: a.seconds }),
  transfer: (a) => ({ type: "transfer", to: a.to }),
  end_call: () => ({ type: "hangup" }),
};

// passJudge passes every criterion.
const passJudge = new Fake([], {
  content: `{"criterion_1":{"reason":"ok","pass":true},"criterion_2":{"reason":"ok","pass":true},"criterion_3":{"reason":"ok","pass":true},"criterion_4":{"reason":"ok","pass":true}}`,
});

function runner(decide: DecideFunc): Runner {
  return new Runner({ llm: fake(), judge: passJudge, decide, handlers });
}

function says(text: string): DecideFunc {
  return async () => [say(text, "neutral", 1)];
}

test("all fixtures load", async () => {
  const fs = await loadFixtures(FIXTURES_DIR);
  assert.ok(fs.length >= 10, `only ${fs.length} fixtures`);
  for (const f of fs) {
    await stat(seedPath(f));
    const last = f.transcript.items.at(-1)!;
    assert.ok(
      last.type === "message" && (last.end ?? 0) > 0 && f.transcript.now >= (last.end ?? 0),
      `${f.id}: last item should be a timed message before now`,
    );
  }
});

// An agent that books what the caller confirmed passes the happy path: the
// diff sees the booking, the invariants hold, no model needed.
test("happy path booking", async () => {
  const book: DecideFunc = async (_tr, api) => {
    await api.createBooking({
      customer_id: "cus_07",
      unit_id: "unit_mazoji",
      start: "2026-10-06",
      end: "2026-10-07",
      people: 4,
      site: { power_available: true },
      tow_capacity_kg: 2000,
    });
    return [say("Done, see you Tuesday.", "warm", 1)];
  };
  const res = await runner(book).run(await load("happy-path"), 1, 1);
  const r = res.runs[0]!;
  assert.ok(res.passed(), `happy path failed: ${r.failures} (err ${r.error})`);
  assert.equal(r.stats.calls, 0, "no model was used");
  assert.equal(res.threshold, 1, "one run means a threshold of one");
});

// The interim fixture wants silence: speech fails it, a wait passes it.
test("interim wants wait", async () => {
  const res = await runner(says("Take your time!")).run(await load("interim-speech"), 1, 1);
  assert.ok(!res.passed(), "speech must not pass interim-speech");
  assert.match(res.runs[0]!.failures.join("\n"), /wanted wait/);

  const waits: DecideFunc = async () => [wait(1.5)];
  const res2 = await runner(waits).run(await load("interim-speech"), 1, 1);
  assert.ok(res2.passed(), `wait should pass: ${res2.runs[0]!.failures}`);
});

// A 422 from the API is a failure even if the reply talks its way out, and
// the failure line says what it was.
test("rule violation is caught", async () => {
  const wrong: DecideFunc = async (_tr, api) => {
    await api
      .createBooking({
        customer_id: "cus_19",
        unit_id: "unit_mazoji",
        start: "2026-10-03",
        end: "2026-10-04",
        people: 7,
        site: { power_available: true },
        tow_capacity_kg: 3500,
      })
      .catch(() => undefined);
    return [say("Sorry, that one is too small. Didžioji then?", "neutral", 1)];
  };
  const res = await runner(wrong).run(await load("capacity"), 1, 1);
  assert.ok(!res.passed(), "a CAPACITY_EXCEEDED in the log must fail the fixture");
  const joined = res.runs[0]!.failures.join("\n");
  assert.match(joined, /CAPACITY_EXCEEDED/);
  assert.match(joined, /rule violation/);
});

// A booking nobody asked for fails a fixture that expects no change.
test("unexpected booking is caught", async () => {
  const eager: DecideFunc = async (_tr, api) => {
    await api.createBooking({
      customer_id: "cus_12",
      unit_id: "unit_didzioji",
      start: "2026-10-10",
      end: "2026-10-11",
      people: 6,
      site: { power_available: true },
      tow_capacity_kg: 2500,
    });
    return [say("Booked the big one.", "neutral", 1)];
  };
  const res = await runner(eager).run(await load("ask-tow-capacity"), 1, 1);
  assert.ok(!res.passed(), "guessing a towing capacity and booking must fail");
  assert.match(res.runs[0]!.failures.join("\n"), /unexpected created/);
});

// Reading another customer's record is a failure; reading the caller's is
// fine. The silence fixture has the caller identified and expects speech and
// no change, so only the invariant decides.
test("identity invariant", async () => {
  const leak: DecideFunc = async (_tr, api) => {
    await api.getCustomer("cus_31");
    return [say("Still there?", "neutral", 1)];
  };
  const res = await runner(leak).run(await load("silence"), 1, 1);
  assert.ok(!res.passed(), "customers.get for someone else must fail");

  const self: DecideFunc = async (_tr, api) => {
    await api.getCustomer("cus_12");
    return [say("Still there?", "neutral", 1)];
  };
  const res2 = await runner(self).run(await load("silence"), 1, 1);
  assert.ok(res2.passed(), `customers.get for the identified caller must pass: ${res2.runs[0]!.failures}`);
});

// A failing verdict fails the run with the reason on the line, and judge
// calls do not count against the agent.
test("judge failure is reported", async () => {
  const failJudge = new Fake([], {
    content: `{"criterion_1":{"reason":"it answered the question instead of asking for a name","pass":false}}`,
  });
  const r = runner(says("Yes, Wednesday is still on."));
  r.judge = failJudge;
  const f = await load("identify-before-reading");
  f.judge = ["The reply asks the caller for their name."];
  const res = await r.run(f, 1, 1);
  assert.ok(!res.passed(), "a failing verdict must fail the run");
  const found = res.runs[0]!.failures.some((fl) => fl.startsWith("judge: ") && fl.includes("instead of asking"));
  assert.ok(found, `want a judge failure with the reason, got ${res.runs[0]!.failures}`);
  assert.equal(res.runs[0]!.stats.calls, 0, "judge calls must not count against the agent");
});

// An action type nobody registered a handler for fails the run, and a handler
// someone adds makes it pass.
test("unknown action needs handler", async () => {
  const decide: DecideFunc = async () => [{ type: "hold_tone", seconds: 2.0 } as Action, say("still there?", "warm", 1)];
  const r = runner(decide);
  const res = await r.run(await load("silence"), 1, 1);
  assert.ok(!res.passed(), "hold_tone has no handler, run must fail");

  r.handlers = { ...handlers, hold_tone: () => ({ type: "hold_tone" }) };
  const res2 = await r.run(await load("silence"), 1, 1);
  assert.ok(res2.passed(), `with a handler the silence fixture should pass: ${res2.runs[0]!.failures}`);
});
