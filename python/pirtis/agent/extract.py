from dataclasses import dataclass, fields
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


def schema(unit_ids: list[str]) -> dict:
    """The JSON Schema for CallerInfo, for structured output."""
    return {
        "type": "object",
        "properties": {
            "intent": {
                "type": "string",
                "enum": ["book", "change", "cancel", "check", "other"],
                "description": "book = new booking, change = move or alter an existing one, "
                "cancel, check = asking about their existing booking, "
                "other = anything else",
            },
            "wants_human": {
                "type": "boolean",
                "description": "Customer wants to speak with a real human employee.",
            },
            "name": {
                "type": ["string", "null"],
                "description": "As the caller said it, first and last name.",
            },
            "people": {
                "type": ["integer", "null"],
                "description": "How many people will use the sauna. "
                "If the caller gives a range ('seven, maybe eight'), "
                "the larger number. Null if not said.",
            },
            "start": {
                "type": ["string", "null"],
                "description": "YYYY-MM-DD, looked up in the calendar. Null if not said. "
                "One day only: start equals end.",
            },
            "end": {
                "type": ["string", "null"],
                "description": "YYYY-MM-DD, looked up in the calendar. Null if not said. "
                "One day only: start equals end.",
            },
            "tow_kg": {
                "type": ["integer", "null"],
                "description": "Braked towing capacity in kg, only if the caller said a number. "
                "'It's fine' or a car model alone is null.",
            },
            "power": {
                "type": ["boolean", "null"],
                "description": "True if there is mains power at the site, false if not. "
                "A generator is not mains. Null if not said.",
            },
            "unit": {
                "type": ["string", "null"],
                "enum": [*unit_ids, None],
                "description": "The unit the caller asked for, by name or description, or null.",
            },
            "confirmed": {
                "type": "boolean",
                "description": "True only if our last line proposed a specific booking and "
                "the caller agreed to it.",
            },
            "answer": {
                "type": "string",
                "description": "A short spoken reply, only if the caller asked something "
                "off-script. Empty string otherwise.",
            },
        },
        "required": [field.name for field in fields(CallerInfo)],
        "additionalProperties": False,
    }
