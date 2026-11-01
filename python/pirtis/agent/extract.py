import json
from dataclasses import dataclass, fields
from datetime import date, timedelta

from pirtis.api.types import Unit
from pirtis.llm.client import Client as LlmClient
from pirtis.llm.client import Message, Request, ResponseFormat
from pirtis.transcript.types import Transcript

_WEEKENDS = {0: "this weekend, next weekend", 1: "the weekend after next"}


def calendar(today: str, days: int = 21) -> str:
    """One line per day from today, for the model to look dates up in. Weekends are
    labelled by code, because the model gets 'this Saturday' wrong on its own."""
    start_day = date.fromisoformat(today)
    monday = start_day - timedelta(days=start_day.weekday())  # Monday of this week
    calendar_lines = []
    for i in range(days):
        day = start_day + timedelta(days=i)
        line = f"{day:%Y-%m-%d %A}"
        if day.weekday() >= 5:  # Saturday is 5, Sunday is 6
            label = _WEEKENDS.get((day - monday).days // 7)
            if label:
                line += f" ({label})"
        calendar_lines.append(line)
    calendar_lines[0] += " (today)"
    return "\n".join(calendar_lines)


@dataclass
class CallerInfo:
    """What the caller has said so far, as read by the model. None means not said."""

    latest: str = ""  # the caller's latest line, quoted, so intent is read from it
    intent: str = "other"  # book | change | cancel | check | staff | goodbye | other
    answer: str = ""  # the reply when intent is other, written right after it
    name: str | None = None  # full name, if the caller gave it
    people: int | None = None  # the larger number if a range
    when: str | None = None  # the caller's own words about the dates, quoted
    start: str | None = None  # YYYY-MM-DD
    end: str | None = None  # YYYY-MM-DD, inclusive
    tow_kg: int | None = None  # only if the caller said a number
    power: bool | None = None  # mains power at the site
    unit: str | None = None  # a unit id like "unit_mazoji", if they asked for one
    confirmed: bool = False  # said yes to our last proposal


def schema(unit_ids: list[str]) -> dict:
    """The JSON Schema for CallerInfo, for structured output."""
    return {
        "type": "object",
        "properties": {
            "latest": {
                "type": "string",
                "description": "The caller's latest line, quoted.",
            },
            "intent": {
                "type": "string",
                "enum": ["book", "change", "cancel", "check", "staff", "goodbye", "other"],
                "description": "book = new booking, change = move or alter an existing one, "
                "cancel, check = asking about their existing booking, "
                "staff = asks to be put through to a person at Pirtis on Wheels, "
                "goodbye = the caller's latest line ends the call ('thanks, that's all, bye'), "
                "even if the call was about a booking, "
                "other = anything else, including questions about other customers",
            },
            "answer": {
                "type": "string",
                "description": "When intent is other: a short spoken reply to the caller's "
                "latest line. Empty string for every other intent.",
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
            "when": {
                "type": ["string", "null"],
                "description": "The caller's own words about the dates, quoted, "
                "e.g. 'this Saturday and Sunday'. Null if not said.",
            },
            "start": {
                "type": ["string", "null"],
                "description": "First day, YYYY-MM-DD, copied from the calendar line that "
                "matches `when`. A weekend is Saturday to Sunday. Null if not said.",
            },
            "end": {
                "type": ["string", "null"],
                "description": "Last day, inclusive, YYYY-MM-DD, copied from the calendar. "
                "Equal to start for a single day. Null if not said.",
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
                "description": "The unit the caller asked for, by name or description "
                "('the big one', 'the electric six seater'). How many people are coming "
                "is not a request for a unit: null.",
            },
            "confirmed": {
                "type": "boolean",
                "description": "True only if our last line proposed a specific booking and "
                "the caller agreed to it.",
            },
        },
        "required": [field.name for field in fields(CallerInfo)],
        "additionalProperties": False,
    }


def history(t: Transcript) -> list[Message]:
    """The spoken lines as chat messages. API calls stay out: code reads those, not the model."""
    messages = []
    for it in t.items:
        if it.type == "message":
            content = it.content + " [cut off]" if it.interrupted else it.content
            messages.append(Message(role=it.role, content=content))
    return messages


def system_prompt(t: Transcript, units: list[Unit]) -> str:
    """Instructions for reading the call. The model fills a form; code decides and speaks."""
    unit_lines = "\n".join(
        f"{u.id}: {u.name}, {u.capacity} people, {'wood-fired' if u.heater == 'wood' else u.heater}"
        for u in units
    )
    return (
        "You read a phone call to Pirtis on Wheels, a mobile sauna rental, "
        "and fill in a form about what the caller wants. Code acts on the form.\n"
        "Only record what the caller actually said. If something was not said, use null.\n\n"
        f"Calendar:\n{calendar(t.today)}\n\n"
        f"Units:\n{unit_lines}\n\n"
        "If the caller asked something off-script, write `answer`: one or two short spoken "
        "sentences, numbers as words. You know nothing about other customers or their "
        "bookings; never guess. Never say a unit is free or booked."
    )


def extract(t: Transcript, units: list[Unit], llm: LlmClient) -> CallerInfo:
    """One model call that reads the call into a CallerInfo. Any failure gives an empty
    CallerInfo, so the caller hears a question instead of silence."""
    req = Request(
        messages=[Message(role="system", content=system_prompt(t, units)), *history(t)],
        temperature=0,
        response_format=ResponseFormat(
            name="caller_info", schema=schema([u.id for u in units]), strict=True
        ),
    )
    try:
        reply = llm.chat(req)
        fields_ = json.loads(reply.content)
    except Exception:
        return CallerInfo()
    # the model sometimes writes the word "null" instead of null
    return CallerInfo(**{k: None if v == "null" else v for k, v in fields_.items()})
