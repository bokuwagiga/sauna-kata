from dataclasses import dataclass
from datetime import date, timedelta


def calendar(today: str, days: int = 21) -> str:
    """One line per day from today, for the model to look dates up in."""
    start_day = date.fromisoformat(today)
    calendar_lines = []
    for i in range(days):
        day = start_day + timedelta(days=i)
        calendar_lines.append(f"{day:%Y-%m-%d %A}")
    calendar_lines[0] += " (today)"
    return "\n".join(calendar_lines)


@dataclass
class CallerInfo:
    """What the caller has said so far, as read by the model. None means not said."""

    intent: str = "other"  # book | change | cancel | check | other
    wants_human: bool = False  # asked for a person, in any words
    name: str | None = None  # full name, if the caller gave it
    people: int | None = None  # the larger number if a range
    start: str | None = None  # YYYY-MM-DD
    end: str | None = None  # YYYY-MM-DD, inclusive
    tow_kg: int | None = None  # only if the caller said a number
    power: bool | None = None  # mains power at the site
    unit: str | None = None  # a unit id like "unit_mazoji", if they asked for one
    confirmed: bool = False  # said yes to our last proposal
    answer: str = ""  # a short reply, only for off-script questions
