"""Runs the fixtures against the agent.

uv run harness                       all fixtures
uv run harness --id silence          one fixture
uv run harness --runs 3 --need 2     three times each, pass on two, for a stability check
uv run harness -v                    print actions, effects and verdicts per run
uv run harness --trace runs          also write every LLM call and per-run results under runs/
"""

import argparse
import json
import os
import sys
from pathlib import Path

from pirtis.agent.actions import handlers
from pirtis.agent.agent import decide
from pirtis.harness.fixture import load_fixtures
from pirtis.harness.run import Runner
from pirtis.jsonutil import json_default
from pirtis.llm.client import open_ai_from_env


def main() -> None:
    p = argparse.ArgumentParser(prog="harness", description="run the fixtures against the agent")
    p.add_argument(
        "--fixtures", default="../fixtures", help="directory with *.yaml fixtures and seeds/"
    )
    p.add_argument(
        "--trace",
        default="",
        help="directory to write every LLM call and per-run results to; off by default",
    )
    p.add_argument("--id", default="", help="comma-separated fixture ids to run; default all")
    p.add_argument("--runs", type=int, default=1, help="runs per fixture")
    p.add_argument(
        "--need", type=int, default=1, help="runs that must pass for the fixture to pass"
    )
    p.add_argument(
        "-v", "--verbose", action="store_true", help="print actions, effects and verdicts per run"
    )
    args = p.parse_args()

    load_dotenv(".env")

    try:
        client = open_ai_from_env()
    except RuntimeError as e:
        print(e, file=sys.stderr)
        print("set LLM_API_KEY, and optionally LLM_BASE_URL; see .env.example", file=sys.stderr)
        sys.exit(2)

    try:
        fixtures = load_fixtures(args.fixtures)
    except (OSError, ValueError) as e:
        print(e, file=sys.stderr)
        sys.exit(2)
    if args.id:
        want = {s.strip() for s in args.id.split(",")}
        fixtures = [f for f in fixtures if f.id in want]
    if not fixtures:
        print("no fixtures matched", file=sys.stderr)
        sys.exit(2)

    runner = Runner(llm=client, decide=decide, handlers=handlers, trace_dir=args.trace)

    passed = 0
    print(_row("fixture", "run", "result", "calls", "tokens in/out", "llm", "wall"))
    for f in fixtures:
        fr = runner.run(f, args.runs, args.need)
        for r in fr.runs:
            print(
                _row(
                    f.id,
                    str(r.index),
                    "PASS" if r.passed() else "FAIL",
                    str(r.stats.calls),
                    f"{r.stats.prompt_tokens}/{r.stats.completion_tokens}",
                    _duration(r.stats.duration_ms),
                    _duration(r.wall_ms),
                )
            )
            if r.error is not None:
                print(f"    error: {r.error}")
            for fl in r.failures:
                print(f"    - {fl}")
            if args.verbose:
                for a in r.actions:
                    print(f"    action: {_json(a)}")
                for ef in r.effects:
                    print(f"    effect: {_json(ef)}")
                for v in r.verdicts:
                    print(f"    judge: {v.passed}  {v.criterion}: {v.reason}")
        if fr.passed():
            passed += 1
        verdict = "pass" if fr.passed() else "FAIL"
        print(f"{'':<26}      {verdict}  ({fr.pass_count()}/{len(fr.runs)}, need {fr.threshold})")
    print(f"\n{passed}/{len(fixtures)} fixtures passed")
    if passed < len(fixtures):
        sys.exit(1)


def _row(fixture: str, run: str, result: str, calls: str, tokens: str, llm: str, wall: str) -> str:
    """One line of the results table."""
    return f"{fixture:<26} {run:<4} {result:<6} {calls:>6} {tokens:>14} {llm:>8} {wall:>8}"


def _duration(ms: int) -> str:
    """Milliseconds the way a person reads them: 530ms, 1.23s."""
    return f"{ms / 1000:.2f}s" if ms >= 1000 else f"{round(ms / 10) * 10}ms"


def _json(o: object) -> str:
    return json.dumps(o, default=json_default, ensure_ascii=False)


def load_dotenv(path: str) -> None:
    """Sets KEY=VALUE lines from path into the environment when the variable
    is not already set. A missing file is fine."""
    try:
        text = Path(path).read_text()
    except OSError:
        return
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        k, v = k.strip(), v.strip().strip("\"'")
        if not os.environ.get(k):
            os.environ[k] = v


if __name__ == "__main__":
    main()
