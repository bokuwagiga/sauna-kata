import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

import type { Handler, HandlerContext } from "../agent/actions.ts";
import { METHOD_CUSTOMERS_IDENTIFY, type Call, type Client as ApiClient } from "../api/client.ts";
import { Memory, loadSeed, type Seed } from "../api/memory.ts";
import type { Booking, IdentifyResult } from "../api/types.ts";
import { Recorder, type Client as LlmClient, type Stats } from "../llm/client.ts";
import type { Action, Effect, Transcript } from "../transcript/types.ts";
import { evaluate } from "./assert.ts";
import { seedPath, setupBooking, type Fixture } from "./fixture.ts";
import { judge, type Verdict } from "./judge.ts";

/** DecideFunc is the agent's entry point. */
export type DecideFunc = (t: Transcript, api: ApiClient, llm: LlmClient) => Promise<Action[]>;

export interface RunnerOptions {
  llm: LlmClient;
  /** grades `judge` criteria; defaults to llm */
  judge?: LlmClient;
  decide: DecideFunc;
  handlers: Record<string, Handler>;
  /** where LLM calls and per-run results are written; empty for nowhere */
  traceDir?: string;
}

/**
 * RunResult is one execution of one fixture: one call to decide, the
 * handlers, the checks.
 */
export class RunResult {
  index: number;
  actions: Action[] = [];
  effects: Effect[] = [];
  failures: string[] = [];
  verdicts: Verdict[] = [];
  error?: Error;
  /** the agent's calls only; the judge is counted separately */
  stats: Stats = { calls: 0, prompt_tokens: 0, completion_tokens: 0, duration_ms: 0, errors: 0 };
  wallMs = 0;
  calls: Call[] = [];

  constructor(index: number) {
    this.index = index;
  }

  /** passed is true when the run produced no failures and no error. */
  passed(): boolean {
    return this.error === undefined && this.failures.length === 0;
  }
}

/** FixtureResult is every run of one fixture. */
export class FixtureResult {
  fixture: Fixture;
  runs: RunResult[] = [];
  /** runs that must pass */
  threshold: number;

  constructor(fixture: Fixture, threshold: number) {
    this.fixture = fixture;
    this.threshold = threshold;
  }

  /** passCount is how many runs passed. */
  passCount(): number {
    return this.runs.filter((r) => r.passed()).length;
  }

  /** passed is true when at least threshold runs passed. */
  passed(): boolean {
    return this.passCount() >= this.threshold;
  }
}

/** Runner runs fixtures against one agent. */
export class Runner {
  llm: LlmClient;
  judge?: LlmClient;
  decide: DecideFunc;
  handlers: Record<string, Handler>;
  traceDir: string;

  constructor(opts: RunnerOptions) {
    this.llm = opts.llm;
    this.judge = opts.judge;
    this.decide = opts.decide;
    this.handlers = opts.handlers;
    this.traceDir = opts.traceDir ?? "";
  }

  /** run executes one fixture `runs` times; `need` of them must pass. */
  async run(f: Fixture, runs = 1, need = 1): Promise<FixtureResult> {
    if (runs <= 0) runs = 1;
    if (need <= 0 || need > runs) need = runs;
    const out = new FixtureResult(f, need);
    for (let i = 1; i <= runs; i++) {
      out.runs.push(await this.runOnce(f, i));
    }
    return out;
  }

  private async runOnce(f: Fixture, i: number): Promise<RunResult> {
    const res = new RunResult(i);

    let seed: Seed;
    try {
      seed = await loadSeed(seedPath(f));
    } catch (err) {
      res.error = wrap("seed", err);
      return res;
    }
    seed.bookings.push(...(f.setup?.bookings ?? []).map(setupBooking));
    const mem = new Memory(seed, f.faults ?? []);
    const before = mem.bookings();

    const tr: Transcript = { ...f.transcript, items: [...f.transcript.items] };

    const dir = this.traceDir ? path.join(this.traceDir, f.id, `run-${i}`) : "";
    const rec = new Recorder(this.llm, dir);

    const start = Date.now();
    let actions: Action[] | undefined;
    let decideErr: unknown;
    try {
      actions = await this.decide(tr, mem, rec);
    } catch (err) {
      decideErr = err;
    }
    res.wallMs = Date.now() - start;
    res.stats = rec.stats();
    if (actions === undefined) {
      res.error = wrap("decide", decideErr);
      res.calls = mem.log();
      await writeResult(dir, f, res, mem.bookings());
      return res;
    }
    res.actions = actions;

    await this.runHandlers(res, { api: mem, transcript: tr });

    res.calls = mem.log();
    const after = mem.bookings();
    res.failures.push(
      ...evaluate(f.expect, {
        effects: res.effects,
        calls: res.calls,
        before,
        after,
        identified: identifiedIn(tr),
      }),
    );

    if (f.judge?.length) await this.runJudge(res, f.judge, tr, dir ? path.join(dir, "judge") : "");

    await writeResult(dir, f, res, after);
    return res;
  }

  /** runHandlers carries out every action and collects the effects; a missing or failing handler is a failure. */
  private async runHandlers(res: RunResult, hc: HandlerContext): Promise<void> {
    for (const a of res.actions) {
      const h = Object.hasOwn(this.handlers, a.type) ? this.handlers[a.type] : undefined;
      if (!h) {
        res.failures.push(`no handler for action type ${JSON.stringify(a.type)}`);
        continue;
      }
      try {
        res.effects.push(await h(a, hc));
      } catch (err) {
        res.failures.push(`handler ${JSON.stringify(a.type)}: ${message(err)}`);
      }
    }
  }

  /** runJudge grades the reply and turns every failed verdict into a failure line. */
  private async runJudge(res: RunResult, criteria: string[], tr: Transcript, dir: string): Promise<void> {
    const { verdicts, error } = await judge(new Recorder(this.judge ?? this.llm, dir), tr, res.effects, criteria);
    res.verdicts = verdicts;
    if (error) res.failures.push(`judge: ${error}`);
    for (const v of verdicts) {
      if (!v.pass) res.failures.push(`judge: ${v.criterion}: ${v.reason}`);
    }
  }
}

/**
 * identifiedIn returns the customer ids a successful customers.identify in
 * the transcript returned.
 */
function identifiedIn(tr: Transcript): Set<string> {
  const names = new Map<string, string>();
  const out = new Set<string>();
  for (const it of tr.items) {
    switch (it.type) {
      case "function_call":
        if (it.call_id) names.set(it.call_id, it.name ?? "");
        break;
      case "function_call_output": {
        if (names.get(it.call_id ?? "") !== METHOD_CUSTOMERS_IDENTIFY) continue;
        const r = tryParse<IdentifyResult>(it.output);
        if (r?.ok && r.customer_id) out.add(r.customer_id);
        break;
      }
    }
  }
  return out;
}

async function writeResult(dir: string, f: Fixture, res: RunResult, bookings: Booking[]): Promise<void> {
  if (!dir) return;
  const out = {
    fixture: f.id,
    run: res.index,
    passed: res.passed(),
    error: res.error?.message,
    failures: res.failures.length ? res.failures : undefined,
    verdicts: res.verdicts.length ? res.verdicts : undefined,
    actions: res.actions,
    effects: res.effects,
    api_calls: res.calls,
    bookings,
    llm: res.stats,
    wall_ms: res.wallMs,
  };
  try {
    await mkdir(dir, { recursive: true });
    await writeFile(path.join(dir, "result.json"), JSON.stringify(out, null, 2));
  } catch {
    // tracing is best effort
  }
}

function tryParse<T>(text: string | undefined): T | undefined {
  try {
    return text === undefined ? undefined : (JSON.parse(text) as T);
  } catch {
    return undefined;
  }
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function wrap(stage: string, err: unknown): Error {
  return new Error(`${stage}: ${message(err)}`, { cause: err });
}
