"""Turns what code decided into what the receptionist says: short sentences,
numbers and dates as words. Code writes every sentence here, so the model can
never offer something the rules do not allow."""

from datetime import date

from pirtis.agent.booking import NoOffer, Offer
from pirtis.agent.extract import CallerInfo
from pirtis.api.types import Booking, Unit

_ONES = (
    "zero one two three four five six seven eight nine ten eleven twelve thirteen "
    "fourteen fifteen sixteen seventeen eighteen nineteen"
).split()
_TENS = "_ _ twenty thirty forty fifty sixty seventy eighty ninety".split()
# ordinals that do not just add "th"
_IRREGULAR = {
    "one": "first",
    "two": "second",
    "three": "third",
    "five": "fifth",
    "eight": "eighth",
    "nine": "ninth",
    "twelve": "twelfth",
}


def number_words(n: int) -> str:
    """0 to 999999 as words: 240 -> 'two hundred and forty'."""
    if n < 20:
        return _ONES[n]
    if n < 100:
        tens, rest = divmod(n, 10)
        return _TENS[tens] + (f"-{_ONES[rest]}" if rest else "")
    if n < 1000:
        hundreds, rest = divmod(n, 100)
        return f"{_ONES[hundreds]} hundred" + (f" and {number_words(rest)}" if rest else "")
    thousands, rest = divmod(n, 1000)
    if not rest:
        return f"{number_words(thousands)} thousand"
    joiner = " and " if rest < 100 else " "
    return f"{number_words(thousands)} thousand{joiner}{number_words(rest)}"


def ordinal_words(n: int) -> str:
    """1 -> 'first', 22 -> 'twenty-second', 30 -> 'thirtieth'."""
    words = number_words(n)
    head, _, last = words.rpartition("-")
    if last in _IRREGULAR:
        last = _IRREGULAR[last]
    elif last.endswith("y"):
        last = last[:-1] + "ieth"
    else:
        last += "th"
    return f"{head}-{last}" if head else last


def day_words(iso: str, month: bool = True) -> str:
    """'2026-10-10' -> 'Saturday the tenth of October'."""
    d = date.fromisoformat(iso)
    words = f"{d:%A} the {ordinal_words(d.day)}"
    return f"{words} of {d:%B}" if month else words


def _range_words(start: str, end: str) -> str:
    """'from Saturday the tenth to Sunday the eleventh of October'; the month is
    said once when both days share it."""
    if start == end:
        return f"on {day_words(start)}"
    same_month = start[:7] == end[:7]
    return f"from {day_words(start, month=not same_month)} to {day_words(end)}"


def _days(start: str, end: str) -> int:
    """How many days a rental covers, both ends included."""
    return (date.fromisoformat(end) - date.fromisoformat(start)).days + 1


def _why_not(u: Unit, info: CallerInfo) -> str:
    """Why we are not offering the unit they asked for."""
    if u.capacity < info.people:
        return f"{u.name} only takes {number_words(u.capacity)}"
    if u.gross_weight_kg > info.tow_kg:
        return f"{u.name} is too heavy for your car"
    if u.needs_power and not info.power:
        return f"{u.name} is electric, so it needs mains power"
    return f"{u.name} isn't free then"


def offer_words(o: Offer, info: CallerInfo) -> str:
    """The offer, with a short reason first for anything we changed."""
    parts = []
    if o.asked_unit is not None:
        parts.append(_why_not(o.asked_unit, info) + ".")
    if o.taken:
        days = " or ".join(day_words(d) for d in o.taken)
        parts.append(f"{o.unit.name} isn't free on {days}.")
    if o.weekend_extended:
        parts.append("Weekend rentals are two days minimum.")
    total = o.unit.price_per_day * _days(o.start, o.end)
    parts.append(
        f"I can do {o.unit.name} {_range_words(o.start, o.end)}, "
        f"that's {number_words(total)} euros. Shall I book it?"
    )
    return " ".join(parts)


def no_offer_words(reason: NoOffer, units: list[Unit]) -> str:
    """What to say when there is nothing to offer yet."""
    if reason == "ask_power":
        return "Is there mains power where the sauna is going?"
    elif reason == "no_fit":
        biggest = max(u.capacity for u in units)
        lightest = min(u.gross_weight_kg for u in units)
        return (
            f"I'm sorry, none of our saunas fits that. The biggest takes "
            f"{number_words(biggest)} people, and the lightest needs a car that tows "
            f"{number_words(lightest)} kilos."
        )
    else:
        return "I'm sorry, nothing that fits is free then. Would other dates work?"


def booked_words(o: Offer) -> str:
    """The confirmation after a booking went through."""
    return f"That's booked: {o.unit.name} {_range_words(o.start, o.end)}. See you then!"


def booking_words(b: Booking, units: list[Unit]) -> str:
    """'Mažoji from Saturday the third to Sunday the fourth of October'."""
    name = next((u.name for u in units if u.id == b.unit_id), "the sauna")
    return f"{name} {_range_words(b.start, b.end)}"
