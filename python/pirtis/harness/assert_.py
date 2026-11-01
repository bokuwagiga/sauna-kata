from dataclasses import dataclass

from pirtis.api import client as api
from pirtis.api.client import Call
from pirtis.api.types import STATUS_BOOKED, Booking, BookingQuery, BookingRequest, IdentifyResult
from pirtis.harness.fixture import BookingExpect, Expect
from pirtis.transcript.types import Effect, action_type


@dataclass
class World:
    """Everything the harness knows after a run."""

    effects: list[Effect]  # from the last turn
    calls: list[Call]  # every API call across all turns
    before: list[Booking]  # bookings when the run started
    after: list[Booking]  # bookings when it ended
    identified: set[str]  # customer ids identified in the transcript before the run


_WANT_EFFECT = {"say": "speech", "wait": "silence", "transfer": "transfer", "end_call": "hangup"}


def evaluate(e: Expect, w: World) -> list[str]:
    """Checks the fixture's expectations and the invariants against the world
    and returns one line per failure. Empty means the run passed.

    Invariants, checked for every fixture:

      - No API call returned 4xx or 5xx, unless the failure was injected. A
        422 means the receptionist offered something the rules do not allow;
        a 400 means the request itself was malformed.
      - Customer data is only touched for an identified caller. customers.get,
        bookings.create, bookings.update and a bookings.list naming a customer
        all require that customer to have been identified, in the transcript
        or by a successful customers.identify earlier in this run.
    """
    fails: list[str] = []

    # action
    types = [action_type(ef) for ef in w.effects]
    if _WANT_EFFECT.get(e.action) not in types:
        fails.append(f"action: wanted {e.action}, got effects {types}")
    if e.action == "wait" and "speech" in types:
        fails.append(f'action: wanted wait, but there was speech: "{_first_speech(w.effects)}"')

    # bookings diff
    changes = _diff(w.before, w.after)
    matched = [False] * len(changes)
    for want in e.bookings:
        for i, c in enumerate(changes):
            if not matched[i] and _matches(want, c):
                matched[i] = True
                break
        else:
            fails.append(
                f"bookings: expected {_expect_str(want)}, got {_describe_changes(changes)}"
            )
    for i, c in enumerate(changes):
        if not matched[i]:
            fails.append(f"bookings: unexpected {c}")

    # invariants
    identified = set(w.identified)
    by_id = {b.id: b for b in w.after}
    for call in w.calls:
        if call.status >= 400 and not call.injected:
            fails.append(
                f"api: {call.method} returned {call.status} {call.code}: {call.detail} "
                f"({_status_meaning(call.status)})"
            )
        match call.method:
            case api.METHOD_CUSTOMERS_IDENTIFY:
                if isinstance(call.result, IdentifyResult) and call.result.ok:
                    identified.add(call.result.customer_id)
            case api.METHOD_CUSTOMERS_GET:
                if str(call.args) not in identified:
                    fails.append(
                        f"api: customers.get({call.args}) before that caller was identified"
                    )
            case api.METHOD_BOOKINGS_CREATE:
                if (
                    isinstance(call.args, BookingRequest)
                    and call.args.customer_id not in identified
                ):
                    fails.append(
                        f"api: bookings.create for {call.args.customer_id} "
                        "before that caller was identified"
                    )
            case api.METHOD_BOOKINGS_UPDATE:
                b = by_id.get(str(call.args.get("id"))) if isinstance(call.args, dict) else None
                if b is not None and b.customer_id not in identified:
                    fails.append(
                        f"api: bookings.update({b.id}) for {b.customer_id} "
                        "before that caller was identified"
                    )
            case api.METHOD_BOOKINGS_LIST:
                if (
                    isinstance(call.args, BookingQuery)
                    and call.args.customer_id
                    and call.args.customer_id not in identified
                ):
                    fails.append(
                        f"api: bookings.list for {call.args.customer_id} "
                        "before that caller was identified"
                    )
    return fails


def _status_meaning(status: int) -> str:
    match status:
        case 400:
            return "malformed request"
        case 404:
            return "no such record"
        case 409:
            return "conflict: dates taken or stale version"
        case 422:
            return "rule violation: offered something the rules do not allow"
        case _:
            return "server error"


@dataclass
class _Change:
    """One created or changed booking."""

    kind: str  # "created" | "changed"
    b: Booking

    def __str__(self) -> str:
        return f"{self.kind} {_describe(self.b)}"


def _describe(b: Booking) -> str:
    return (
        f"{b.id}({b.customer_id} {b.unit_id} {b.start}..{b.end} "
        f"x{b.people} {b.status} v{b.version})"
    )


def _describe_changes(cs: list[_Change]) -> str:
    return ", ".join(str(c) for c in cs) if cs else "no changes"


def _diff(before: list[Booking], after: list[Booking]) -> list[_Change]:
    prev = {b.id: b for b in before}
    out = []
    for b in after:
        p = prev.get(b.id)
        if p is None:
            out.append(_Change("created", b))
        elif p != b:
            out.append(_Change("changed", b))
    return out


def _matches(e: BookingExpect, c: _Change) -> bool:
    b = c.b
    if e.id and (e.id != b.id or c.kind != "changed"):
        return False
    if not e.id and c.kind != "created":
        return False
    if e.customer and e.customer != b.customer_id:
        return False
    if e.unit and e.unit != b.unit_id:
        return False
    if e.start and e.start != b.start:
        return False
    if e.end and e.end != b.end:
        return False
    if e.people is not None and e.people != b.people:
        return False
    return (e.status or STATUS_BOOKED) == b.status


def _expect_str(e: BookingExpect) -> str:
    parts = []
    if e.id:
        parts.append("id=" + e.id)
    if e.customer:
        parts.append("customer=" + e.customer)
    if e.unit:
        parts.append("unit=" + e.unit)
    if e.start:
        parts.append("start=" + e.start)
    if e.end:
        parts.append("end=" + e.end)
    if e.people is not None:
        parts.append(f"people={e.people}")
    if e.status:
        parts.append("status=" + e.status)
    kind = "changed" if e.id else "created"
    return kind + "{" + " ".join(parts) + "}"


def _first_speech(effects: list[Effect]) -> str:
    for ef in effects:
        if action_type(ef) == "speech":
            return str(ef.get("text"))
    return ""
