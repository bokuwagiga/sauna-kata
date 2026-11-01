import json
import time
from collections.abc import Callable
from dataclasses import dataclass, field, replace
from pathlib import Path

from pirtis.agent.actions import Handler, HandlerContext
from pirtis.api.client import METHOD_CUSTOMERS_IDENTIFY, Call
from pirtis.api.client import Client as ApiClient
from pirtis.api.memory import Memory, load_seed
from pirtis.api.types import Booking
from pirtis.harness.assert_ import World, evaluate
from pirtis.harness.fixture import Fixture
from pirtis.harness.judge import Verdict, judge
from pirtis.jsonutil import json_default
from pirtis.llm.client import Client as LlmClient
from pirtis.llm.client import Recorder, Stats
from pirtis.transcript.types import Action, Effect, Transcript, action_type

# DecideFunc is the agent's entry point.
DecideFunc = Callable[[Transcript, ApiClient, LlmClient], list[Action]]


@dataclass
class RunResult:
    """One execution of one fixture: one call to decide, the handlers, the
    checks."""

    index: int
    actions: list[Action] = field(default_factory=list)
    effects: list[Effect] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)
    verdicts: list[Verdict] = field(default_factory=list)
    error: str | None = None
    stats: Stats = field(
        default_factory=Stats
    )  # the agent's calls only; the judge is counted separately
    wall_ms: int = 0
    calls: list[Call] = field(default_factory=list)

    def passed(self) -> bool:
        """True when the run produced no failures and no error."""
        return self.error is None and not self.failures


@dataclass
class FixtureResult:
    """Every run of one fixture."""

    fixture: Fixture
    threshold: int  # runs that must pass
    runs: list[RunResult] = field(default_factory=list)

    def pass_count(self) -> int:
        return sum(1 for r in self.runs if r.passed())

    def passed(self) -> bool:
        """True when at least threshold runs passed."""
        return self.pass_count() >= self.threshold


@dataclass
class Runner:
    """Runs fixtures against one agent."""

    llm: LlmClient
    decide: DecideFunc
    handlers: dict[str, Handler]
    judge: LlmClient | None = None  # grades `judge` criteria; None means llm
    trace_dir: str = ""  # where LLM calls and per-run results are written; "" for nowhere

    def run(self, f: Fixture, runs: int = 1, need: int = 1) -> FixtureResult:
        """Executes one fixture `runs` times; `need` of them must pass."""
        runs = max(runs, 1)
        if need <= 0 or need > runs:
            need = runs
        out = FixtureResult(fixture=f, threshold=need)
        for i in range(1, runs + 1):
            out.runs.append(self._run_once(f, i))
        return out

    def _run_once(self, f: Fixture, i: int) -> RunResult:
        res = RunResult(index=i)

        try:
            seed = load_seed(f.seed_path())
        except (OSError, ValueError) as e:
            res.error = f"seed: {e}"
            return res
        seed.bookings.extend(sb.booking() for sb in f.setup.bookings)
        mem = Memory(seed, f.faults)
        before = mem.bookings()

        tr = replace(f.transcript, items=list(f.transcript.items))

        dir = Path(self.trace_dir) / f.id / f"run-{i}" if self.trace_dir else None
        rec = Recorder(self.llm, dir)

        start = time.perf_counter()
        try:
            actions = self.decide(tr, mem, rec)
        except Exception as e:
            res.error = f"decide: {e}"
            actions = []
        res.wall_ms = round((time.perf_counter() - start) * 1000)
        res.stats = rec.stats()
        res.actions = actions
        if res.error is not None:
            res.calls = mem.log()
            self._write_result(dir, f, res, mem.bookings())
            return res

        hc = HandlerContext(api=mem, transcript=tr)
        for a in actions:
            h = self.handlers.get(action_type(a))
            if h is None:
                res.failures.append(f'no handler for action type "{action_type(a)}"')
                continue
            try:
                res.effects.append(h(a, hc))
            except Exception as e:
                res.failures.append(f'handler "{action_type(a)}": {e}')

        res.calls = mem.log()
        after = mem.bookings()
        res.failures.extend(
            evaluate(
                f.expect,
                World(
                    effects=res.effects,
                    calls=res.calls,
                    before=before,
                    after=after,
                    identified=identified_in(tr),
                ),
            )
        )

        if f.judge:
            jdir = dir / "judge" if dir else None
            try:
                res.verdicts = judge(
                    Recorder(self.judge or self.llm, jdir), tr, res.effects, f.judge
                )
            except Exception as e:
                res.failures.append(f"judge: {e}")
            for v in res.verdicts:
                if not v.passed:
                    res.failures.append(f"judge: {v.criterion}: {v.reason}")

        self._write_result(dir, f, res, after)
        return res

    def _write_result(
        self, dir: Path | None, f: Fixture, res: RunResult, bookings: list[Booking]
    ) -> None:
        if dir is None:
            return
        out = {
            "fixture": f.id,
            "run": res.index,
            "passed": res.passed(),
            **({"error": res.error} if res.error else {}),
            **({"failures": res.failures} if res.failures else {}),
            **({"verdicts": res.verdicts} if res.verdicts else {}),
            "actions": res.actions,
            "effects": res.effects,
            "api_calls": res.calls,
            "bookings": bookings,
            "llm": res.stats,
            "wall_ms": res.wall_ms,
        }
        try:
            dir.mkdir(parents=True, exist_ok=True)
            (dir / "result.json").write_text(
                json.dumps(out, indent=2, default=json_default, ensure_ascii=False)
            )
        except OSError:
            pass


def identified_in(tr: Transcript) -> set[str]:
    """The customer ids a successful customers.identify in the transcript
    returned."""
    names: dict[str, str] = {}
    out: set[str] = set()
    for it in tr.items:
        match it.type:
            case "function_call":
                names[it.call_id] = it.name
            case "function_call_output" if names.get(it.call_id) == METHOD_CUSTOMERS_IDENTIFY:
                try:
                    r = json.loads(it.output)
                except ValueError:
                    continue
                if isinstance(r, dict) and r.get("ok") and r.get("customer_id"):
                    out.add(str(r["customer_id"]))
    return out
