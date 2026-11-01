// The harness command runs the fixtures against the agent.
//
//   node src/cmd/harness.ts                   all fixtures
//   node src/cmd/harness.ts --id silence      one fixture
//   node src/cmd/harness.ts --runs 3 --need 2 three times each, pass on two, for a stability check
//   node src/cmd/harness.ts -v                print actions, effects and verdicts per run
//   node src/cmd/harness.ts --trace runs      also write every LLM call and per-run results under runs/

import { parseArgs } from "node:util";

import { decide } from "../agent/agent.ts";
import { handlers } from "../agent/actions.ts";
import { loadFixtures } from "../harness/fixture.ts";
import { Runner } from "../harness/run.ts";
import { openAIFromEnv, type OpenAI } from "../llm/client.ts";

const USAGE = `usage: node src/cmd/harness.ts [options]

  --fixtures <dir>   directory with *.yaml fixtures and seeds/ (default ../fixtures)
  --trace <dir>      directory to write every LLM call and per-run results to; off by default
  --id <ids>         comma-separated fixture ids to run; default all
  --runs <n>         runs per fixture (default 1)
  --need <n>         runs that must pass for the fixture to pass (default 1)
  -v, --verbose      print actions, effects and verdicts per run
  -h, --help         show this help
`;

function fail(msg: string): never {
  console.error(msg);
  process.exit(2);
}

function integer(name: string, raw: string): number {
  const n = Number(raw);
  if (!Number.isInteger(n)) fail(`--${name}: want an integer, got ${JSON.stringify(raw)}`);
  return n;
}

const flags = parseFlags();
if (flags.help) {
  process.stdout.write(USAGE);
  process.exit(0);
}
const runs = integer("runs", flags.runs);
const need = integer("need", flags.need);

loadDotEnv(".env");

let client: OpenAI;
try {
  client = openAIFromEnv();
} catch (err) {
  console.error((err as Error).message);
  fail("set LLM_API_KEY, and optionally LLM_BASE_URL; see .env.example");
}

let fixtures = await loadFixtures(flags.fixtures).catch((err: Error) => fail(err.message));
if (flags.id) {
  const want = new Set(flags.id.split(",").map((s) => s.trim()));
  fixtures = fixtures.filter((f) => want.has(f.id));
}
if (fixtures.length === 0) fail("no fixtures matched");

const runner = new Runner({ llm: client, decide, handlers, traceDir: flags.trace });

let passed = 0;
console.log(row("fixture", "run", "result", "calls", "tokens in/out", "llm", "wall"));
for (const f of fixtures) {
  const fr = await runner.run(f, runs, need);
  for (const r of fr.runs) {
    console.log(
      row(
        f.id,
        String(r.index),
        r.passed() ? "PASS" : "FAIL",
        String(r.stats.calls),
        `${r.stats.prompt_tokens}/${r.stats.completion_tokens}`,
        duration(r.stats.duration_ms),
        duration(r.wallMs),
      ),
    );
    if (r.error) console.log(`    error: ${r.error.message}`);
    for (const fl of r.failures) console.log(`    - ${fl}`);
    if (flags.verbose) {
      for (const a of r.actions) console.log(`    action: ${JSON.stringify(a)}`);
      for (const e of r.effects) console.log(`    effect: ${JSON.stringify(e)}`);
      for (const v of r.verdicts) console.log(`    judge: ${v.pass}  ${v.criterion}: ${v.reason}`);
    }
  }
  if (fr.passed()) passed++;
  const verdict = fr.passed() ? "pass" : "FAIL";
  console.log(`${"".padEnd(26)}      ${verdict}  (${fr.passCount()}/${fr.runs.length}, need ${fr.threshold})`);
}
console.log(`\n${passed}/${fixtures.length} fixtures passed`);
if (passed < fixtures.length) process.exitCode = 1;

function parseFlags() {
  try {
    return parseArgs({
      options: {
        fixtures: { type: "string", default: "../fixtures" },
        trace: { type: "string", default: "" },
        id: { type: "string", default: "" },
        runs: { type: "string", default: "1" },
        need: { type: "string", default: "1" },
        verbose: { type: "boolean", short: "v", default: false },
        help: { type: "boolean", short: "h", default: false },
      },
    }).values;
  } catch (err) {
    return fail(`${(err as Error).message}\n\n${USAGE}`);
  }
}

/** row lays out one line of the results table. */
function row(fixture: string, run: string, result: string, calls: string, tokens: string, llm: string, wall: string) {
  return [
    fixture.padEnd(26),
    run.padEnd(4),
    result.padEnd(6),
    calls.padStart(6),
    tokens.padStart(14),
    llm.padStart(8),
    wall.padStart(8),
  ].join(" ");
}

/** duration prints milliseconds the way a person reads them: 530ms, 1.23s. */
function duration(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)}s` : `${Math.round(ms / 10) * 10}ms`;
}

/**
 * loadDotEnv sets KEY=VALUE lines from path into the environment when the
 * variable is not already set. A missing file is fine.
 */
function loadDotEnv(path: string): void {
  try {
    process.loadEnvFile(path);
  } catch {
    // no .env; the variables may be set already
  }
}
