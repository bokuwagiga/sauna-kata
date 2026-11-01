"""Loads fixtures, seeds the API, calls the agent with a transcript, runs the
handlers and checks the world afterwards."""

import re
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

from pirtis.api.client import Fault
from pirtis.api.types import STATUS_BOOKED, Booking, Site
from pirtis.jsonutil import build
from pirtis.transcript.types import Transcript


class _Loader(yaml.SafeLoader):
    """SafeLoader without YAML 1.1 surprises: `on` stays a string instead of
    becoming True, and unquoted dates stay strings. Only true/false are
    booleans."""


_Loader.yaml_implicit_resolvers = {
    ch: [
        (tag, rx)
        for tag, rx in rs
        if tag not in ("tag:yaml.org,2002:bool", "tag:yaml.org,2002:timestamp")
    ]
    for ch, rs in yaml.SafeLoader.yaml_implicit_resolvers.items()
}
_Loader.add_implicit_resolver("tag:yaml.org,2002:bool", re.compile(r"^(?:true|false)$"), list("tf"))


@dataclass
class SetupBooking:
    """A booking that already exists when the turn starts."""

    id: str
    customer: str
    unit: str
    start: str
    end: str
    people: int
    power: bool = False
    tow: int = 0
    version: int = 0
    total: int = 0

    def booking(self) -> Booking:
        return Booking(
            customer_id=self.customer,
            unit_id=self.unit,
            start=self.start,
            end=self.end,
            people=self.people,
            site=Site(power_available=self.power),
            tow_capacity_kg=self.tow,
            id=self.id,
            version=self.version or 1,
            status=STATUS_BOOKED,
            total=self.total,
        )


@dataclass
class Setup:
    """Rows added to the base seed before the run."""

    bookings: list[SetupBooking] = field(default_factory=list)


@dataclass
class BookingExpect:
    """One expected created or changed booking."""

    id: str = ""
    customer: str = ""
    unit: str = ""
    start: str = ""
    end: str = ""
    people: int | None = None
    status: str = ""


@dataclass
class Expect:
    """What must be true after the last turn. Everything not listed here is
    an invariant the harness always checks; see evaluate."""

    # The effect type the last turn must produce: say, wait, transfer or
    # end_call. wait also means no speech.
    action: str = ""
    # What changed in the bookings table, as a diff against the seed. Empty
    # means nothing changed. Listed fields are compared, the rest are
    # ignored. id names an existing booking that must have changed.
    bookings: list[BookingExpect] = field(default_factory=list)


@dataclass
class Fixture:
    """One test case: the call so far, what the world must look like after
    the agent's turn, and optionally what a judge should check in the reply.
    One YAML file each, one turn each: decide is called once with the
    transcript and the checks run on what it did."""

    id: str
    expect: Expect
    transcript: Transcript
    setup: Setup = field(default_factory=Setup)  # rows added to the base seed
    faults: list[Fault] = field(default_factory=list)  # make the nth call of a method fail
    # A list of statements about the reply, each true or false, for fixtures
    # where the right answer is a sentence and not a booking. The same pinned
    # model reads the call and the reply at temperature zero and marks each
    # one with a one-sentence reason; a failed line prints as
    # "judge: <statement>: <reason>". Optional.
    judge: list[str] = field(default_factory=list)
    dir: Path = Path(".")  # where the fixture file lives

    @classmethod
    def from_dict(cls, d: Mapping[str, Any], dir: Path) -> "Fixture":
        expect = d.get("expect") or {}
        setup = d.get("setup") or {}
        return cls(
            id=str(d.get("id") or ""),
            expect=Expect(
                action=str(expect.get("action") or ""),
                bookings=[build(BookingExpect, b) for b in expect.get("bookings") or []],
            ),
            transcript=Transcript.from_dict(d.get("transcript") or {}),
            setup=Setup(bookings=[build(SetupBooking, b) for b in setup.get("bookings") or []]),
            faults=[build(Fault, f) for f in d.get("faults") or []],
            judge=[str(j) for j in d.get("judge") or []],
            dir=dir,
        )

    def seed_path(self) -> Path:
        """The base seed every fixture starts from."""
        return self.dir / "seeds" / "base.json"


def load_fixtures(dir: str | Path) -> list[Fixture]:
    """Reads every *.yaml in dir, sorted by file name."""
    dir = Path(dir)
    out = []
    for p in sorted(dir.glob("*.yaml")):
        try:
            d = yaml.load(p.read_text(), Loader=_Loader)
        except yaml.YAMLError as e:
            raise ValueError(f"{p}: {e}") from e
        f = Fixture.from_dict(d or {}, dir)
        if not f.id:
            raise ValueError(f"{p}: missing id")
        if not f.transcript.items or not f.transcript.today:
            raise ValueError(f"{p}: transcript needs today and items")
        if f.expect.action not in ("say", "wait", "transfer", "end_call"):
            raise ValueError(f"{p}: expect.action must be say, wait, transfer or end_call")
        out.append(f)
    return out
