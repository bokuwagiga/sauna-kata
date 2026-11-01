"""These tests drive the harness with stub agents, never with the shipped
agent, so a candidate's rewrite of pirtis/agent cannot break them."""

from pathlib import Path

import pytest

from pirtis.agent.actions import Handler, HandlerContext
from pirtis.api.types import BookingRequest, Site
from pirtis.harness.fixture import Fixture, load_fixtures
from pirtis.harness.run import DecideFunc, Runner
from pirtis.llm.client import Response
from pirtis.llm.fake import Fake
from pirtis.transcript.types import Action, Effect, say, wait

FIXTURES_DIR = Path(__file__).resolve().parents[3] / "fixtures"


def load(id: str) -> Fixture:
    for f in load_fixtures(FIXTURES_DIR):
        if f.id == id:
            return f
    pytest.fail(f'no fixture "{id}"')


# the effect contract the harness checks against: say -> speech,
# wait -> silence, transfer -> transfer, end_call -> hangup.
HANDLERS: dict[str, Handler] = {
    "say": lambda a, _: {"type": "speech", "text": a.get("text")},
    "wait": lambda a, _: {"type": "silence", "seconds": a.get("seconds")},
    "transfer": lambda a, _: {"type": "transfer", "to": a.get("to")},
    "end_call": lambda a, _: {"type": "hangup"},
}

# passes every criterion
PASS_JUDGE = Fake(
    default=Response(
        content='{"criterion_1":{"reason":"ok","pass":true},"criterion_2":{"reason":"ok","pass":true},'
        '"criterion_3":{"reason":"ok","pass":true},"criterion_4":{"reason":"ok","pass":true}}'
    )
)


def runner(decide: DecideFunc) -> Runner:
    return Runner(llm=Fake(), judge=PASS_JUDGE, decide=decide, handlers=HANDLERS)


def says(text: str) -> DecideFunc:
    return lambda t, api, llm: [say(text, "neutral", 1)]


def test_all_fixtures_load():
    fs = load_fixtures(FIXTURES_DIR)
    assert len(fs) >= 10
    for f in fs:
        assert f.seed_path().exists(), f.id
        last = f.transcript.items[-1]
        assert last.type == "message" and last.end > 0 and f.transcript.now >= last.end, (
            f"{f.id}: last item should be a timed message before now, "
            f"got {last} (now {f.transcript.now})"
        )


def test_happy_path_booking():
    """An agent that books what the caller confirmed passes the happy path:
    the diff sees the booking, the invariants hold, no model needed."""

    def book(t, api, llm):
        api.create_booking(
            BookingRequest(
                customer_id="cus_07",
                unit_id="unit_mazoji",
                start="2026-10-06",
                end="2026-10-07",
                people=4,
                site=Site(power_available=True),
                tow_capacity_kg=2000,
            )
        )
        return [say("Done, see you Tuesday.", "warm", 1)]

    res = runner(book).run(load("happy-path"), 1, 1)
    r = res.runs[0]
    assert res.passed(), f"happy path failed: {r.failures} (err {r.error})"
    assert r.stats.calls == 0, "no model was used"
    assert res.threshold == 1


def test_interim_wants_wait():
    """The interim fixture wants silence: speech fails it, a wait passes it."""
    res = runner(says("Take your time!")).run(load("interim-speech"), 1, 1)
    assert not res.passed(), "speech must not pass interim-speech"
    assert "wanted wait" in "\n".join(res.runs[0].failures)

    res = runner(lambda t, api, llm: [wait(1.5)]).run(load("interim-speech"), 1, 1)
    assert res.passed(), f"wait should pass: {res.runs[0].failures}"


def test_rule_violation_is_caught():
    """A 422 from the API is a failure even if the reply talks its way out,
    and the failure line says what it was."""

    def wrong(t, api, llm):
        try:
            api.create_booking(
                BookingRequest(
                    customer_id="cus_19",
                    unit_id="unit_mazoji",
                    start="2026-10-03",
                    end="2026-10-04",
                    people=7,
                    site=Site(power_available=True),
                    tow_capacity_kg=3500,
                )
            )
        except Exception:
            pass
        return [say("Sorry, that one is too small. Didžioji then?", "neutral", 1)]

    res = runner(wrong).run(load("capacity"), 1, 1)
    assert not res.passed(), "a CAPACITY_EXCEEDED in the log must fail the fixture"
    joined = "\n".join(res.runs[0].failures)
    assert "CAPACITY_EXCEEDED" in joined and "rule violation" in joined, res.runs[0].failures


def test_unexpected_booking_is_caught():
    """A booking nobody asked for fails a fixture that expects no change."""

    def eager(t, api, llm):
        api.create_booking(
            BookingRequest(
                customer_id="cus_12",
                unit_id="unit_didzioji",
                start="2026-10-10",
                end="2026-10-11",
                people=6,
                site=Site(power_available=True),
                tow_capacity_kg=2500,
            )
        )
        return [say("Booked the big one.", "neutral", 1)]

    res = runner(eager).run(load("ask-tow-capacity"), 1, 1)
    assert not res.passed(), "guessing a towing capacity and booking must fail"
    assert "unexpected created" in "\n".join(res.runs[0].failures), res.runs[0].failures


def test_identity_invariant():
    """Reading another customer's record is a failure; reading the caller's
    is fine. The silence fixture has the caller identified and expects speech
    and no change, so only the invariant decides."""

    def leak(t, api, llm):
        api.get_customer("cus_31")
        return [say("Still there?", "neutral", 1)]

    assert not runner(leak).run(load("silence"), 1, 1).passed(), (
        "customers.get for someone else must fail"
    )

    def self_(t, api, llm):
        api.get_customer("cus_12")
        return [say("Still there?", "neutral", 1)]

    res = runner(self_).run(load("silence"), 1, 1)
    assert res.passed(), (
        f"customers.get for the identified caller must pass: {res.runs[0].failures}"
    )


def test_judge_failure_is_reported():
    """A failing verdict fails the run with the reason on the line, and judge
    calls do not count against the agent."""
    fail_judge = Fake(
        default=Response(
            content='{"criterion_1":{"reason":"it answered the question instead of asking '
            'for a name","pass":false}}'
        )
    )
    r = runner(says("Yes, Wednesday is still on."))
    r.judge = fail_judge
    f = load("identify-before-reading")
    f.judge = ["The reply asks the caller for their name."]
    res = r.run(f, 1, 1)
    assert not res.passed(), "a failing verdict must fail the run"
    assert any(
        fl.startswith("judge: ") and "instead of asking" in fl for fl in res.runs[0].failures
    ), res.runs[0].failures
    assert res.runs[0].stats.calls == 0, "judge calls must not count against the agent"


def test_unknown_action_needs_handler():
    """An action type nobody registered a handler for fails the run, and a
    handler someone adds makes it pass."""

    def decide(t, api, llm) -> list[Action]:
        return [{"type": "hold_tone", "seconds": 2.0}, say("still there?", "warm", 1)]

    r = runner(decide)
    assert not r.run(load("silence"), 1, 1).passed(), "hold_tone has no handler, run must fail"

    def hold_tone(a: Action, hc: HandlerContext) -> Effect:
        return {"type": "hold_tone"}

    r.handlers = {**HANDLERS, "hold_tone": hold_tone}
    res = r.run(load("silence"), 1, 1)
    assert res.passed(), f"with a handler the silence fixture should pass: {res.runs[0].failures}"
