from dataclasses import dataclass, replace
from datetime import date, timedelta
from typing import Literal

from pirtis.agent.extract import CallerInfo
from pirtis.api.client import Client as ApiClient
from pirtis.api.types import AvailabilityQuery, DateRange, Unit

_SEARCH = timedelta(days=7)  # how far either way we look for other dates


def missing_question(info: CallerInfo) -> str | None:
    """The one question to ask next for a booking, or None when we have enough to search."""
    if info.start is None:
        return "Which dates were you thinking of?"
    if info.people is None:
        return "How many people will be using it?"
    if info.tow_kg is None:
        return (
            "Just so I pick one your car can pull, "
            "do you know its braked towing capacity, in kilos?"
        )
    return None


def weekend_end(start: str, end: str | None) -> str:
    """The end date to offer: one day if none was given, and two days if a
    one-day rental falls on a Saturday or Sunday."""
    if end is None:
        end = start
    if start != end:
        return end  # two days or more always meets the minimum
    day = date.fromisoformat(start)
    if day.weekday() >= 5:
        day = day + timedelta(days=1)
    return day.isoformat()


@dataclass
class Offer:
    """A unit and dates the rules allow and that are free right now. The flags say
    what we changed from the caller's request, so the reply can explain it."""

    unit: Unit
    start: str
    end: str
    weekend_extended: bool = False  # a one-day weekend rental made two days
    dates_moved: bool = False  # their dates were taken for this unit
    asked_unit: Unit | None = None  # the unit they asked for, when we offer another


# Why there is no offer: we need to ask about power, no unit fits, or none is free.
NoOffer = Literal["ask_power", "no_fit", "not_free"]


def _fits(u: Unit, info: CallerInfo) -> bool:
    """The unit takes the group, the car can tow it, and the site can power it.
    Call only after missing_question returned None"""
    return (
        u.capacity >= info.people
        and u.gross_weight_kg <= info.tow_kg
        and (info.power is True or not u.needs_power)
    )


def _power_question_needed(info: CallerInfo, units: list[Unit], asked: Unit | None) -> bool:
    """Power is unknown, and it matters: they asked for an electric unit that would
    otherwise fit, or only an electric unit fits."""
    if info.power is not None:
        return False
    with_power = replace(info, power=True)
    if asked is not None and asked.needs_power and _fits(asked, with_power):
        return True
    return not any(_fits(u, info) for u in units) and any(_fits(u, with_power) for u in units)


def _free_ranges(api: ApiClient, unit_id: str, start: str, end: str) -> list[DateRange]:
    """The free ranges of one unit between start and end. The API has already
    removed the turnaround day around every booking."""
    for ua in api.availability(AvailabilityQuery(from_=start, to=end)):
        if ua.unit_id == unit_id:
            return ua.free
    return []


def _is_free(api: ApiClient, unit_id: str, start: str, end: str) -> bool:
    """Every day from start to end is free for this unit"""
    return any(r.start <= start and end <= r.end for r in _free_ranges(api, unit_id, start, end))


def _nearby(
    api: ApiClient, unit_id: str, start: str, end: str, today: str
) -> tuple[str, str] | None:
    """The closest dates of the same length, up to a week either way when this
    unit is free. Later dates win a tie: the earlier ones may already be past."""
    s, e = date.fromisoformat(start), date.fromisoformat(end)
    earliest = max(s - _SEARCH, date.fromisoformat(today))
    free = _free_ranges(api, unit_id, earliest.isoformat(), (e + _SEARCH).isoformat())
    for n in range(1, _SEARCH.days + 1):
        for shift in (timedelta(days=n), timedelta(days=-n)):
            new_start = (s + shift).isoformat()
            new_end = weekend_end(new_start, (e + shift).isoformat())
            if new_start < earliest.isoformat():
                continue
            if any(r.start <= new_start and new_end <= r.end for r in free):
                return new_start, new_end
    return None


def find_offer(info: CallerInfo, units: list[Unit], api: ApiClient, today: str) -> Offer | NoOffer:
    """What to offer for a complete request. In order of preference: the unit they
    asked for on their dates, that unit on the nearest free dates, then the smallest
    other unit that fits their dates."""
    end = weekend_end(info.start, info.end)
    extended = end != (info.end or info.start)
    asked = next((u for u in units if u.id == info.unit), None)

    if _power_question_needed(info, units, asked):
        return "ask_power"
    fitting = sorted((u for u in units if _fits(u, info)), key=lambda u: u.capacity)
    if not fitting:
        return "no_fit"

    if asked in fitting:
        if _is_free(api, asked.id, info.start, end):
            return Offer(asked, info.start, end, weekend_extended=extended)
        moved = _nearby(api, asked.id, info.start, end, today)
        if moved is not None:
            return Offer(asked, *moved, weekend_extended=extended, dates_moved=True)
    for u in fitting:
        if u is not asked and _is_free(api, u.id, info.start, end):
            return Offer(u, info.start, end, weekend_extended=extended, asked_unit=asked)
    return "not_free"
