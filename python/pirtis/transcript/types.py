"""The shapes the harness feeds the agent and the shapes the agent returns.

Field names follow what voice stacks already use: role and item type from the
OpenAI Realtime API, is_final and speech_final from Deepgram, interrupted from
LiveKit, prosody from Hume, and generation_config on the output from Cartesia.
"""

import json
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from pirtis.jsonutil import build


@dataclass
class Emotion:
    """One named dimension and its score in [0, 1]."""

    name: str
    score: float


@dataclass
class Prosody:
    """A rough read of how the line was said, not what was said."""

    emotions: list[Emotion] = field(default_factory=list)


@dataclass
class Caller:
    """What the phone system knows before anyone speaks."""

    phone: str = ""


@dataclass
class Item:
    """One conversation item. Type decides which fields are set.

        message               role, start, end, content, and either interrupted
                              (assistant) or is_final, speech_final, confidence,
                              prosody (user)
        function_call         call_id, name, arguments, start
        function_call_output  call_id, output, end

    Function calls are API calls made earlier in the call, named by API method
    ("customers.identify", "bookings.create"), with the request as arguments
    and the response, or {"error", "detail"}, as output.
    """

    type: str
    role: str = ""  # "user" | "assistant"
    start: float = 0.0
    end: float = 0.0
    content: str = ""
    interrupted: bool = False  # assistant: cut off by a barge-in
    is_final: bool = False  # user: transcript text is stable
    speech_final: bool = False  # user: the speaker paused
    confidence: float = 0.0
    prosody: Prosody | None = None

    call_id: str = ""
    name: str = ""
    arguments: str = ""  # JSON text
    output: str = ""  # JSON text

    @classmethod
    def from_dict(cls, d: Mapping[str, Any]) -> "Item":
        it = build(cls, {**d, "prosody": None})
        it.start, it.end, it.confidence = float(it.start), float(it.end), float(it.confidence)
        if d.get("prosody"):
            it.prosody = Prosody([build(Emotion, e) for e in d["prosody"].get("emotions", [])])
        return it


@dataclass
class Transcript:
    """The call so far, as of now.

    Reading it:

      - A user item with is_final False is text still changing; is_final True
        and speech_final False is stable text from someone still talking;
        speech_final True means the speaker paused.
      - A large gap between now and the last item's end, with speech_final
        True, is silence.
      - A user item whose start falls inside an assistant item's range is a
        barge-in; that assistant item carries interrupted True.
      - Two user items in a row is a caller who paused and went on.
      - function_call and function_call_output items are API calls made
        earlier in the call, named by API method ("customers.identify",
        "bookings.create"), with the request as arguments and the response,
        or {"error", "detail"}, as output. Whatever tools you expose to the
        model, the transcript records what reached the API.

    Callers are people. They hedge, restart, correct themselves, talk to
    someone in the room, and change the plan halfway.
    """

    call_id: str = ""
    today: str = ""  # "2026-09-28", fixed per fixture so date words resolve
    now: float = 0.0  # seconds since the call started
    caller: Caller = field(default_factory=Caller)
    items: list[Item] = field(default_factory=list)

    @classmethod
    def from_dict(cls, d: Mapping[str, Any]) -> "Transcript":
        return cls(
            call_id=str(d.get("call_id", "")),
            today=str(d.get("today", "")),
            now=float(d.get("now", 0.0)),
            caller=build(Caller, d.get("caller") or {}),
            items=[Item.from_dict(it) for it in d.get("items") or []],
        )


# Action is what the receptionist does next. It is a dict with a "type" key so
# new action types and new fields need no schema change. The harness runs each
# action through the handler registered for its type; an action with no
# handler fails the fixture. say, wait, transfer and end_call below build the
# four the shipped handlers know; build others by hand.
Action = dict[str, Any]

# Effect is what a handler produced when it carried out an action. The
# fixtures assert on effects.
Effect = dict[str, Any]


def action_type(a: Mapping[str, Any]) -> str:
    """The "type" key of an action or effect, or "" if missing."""
    t = a.get("type")
    return t if isinstance(t, str) else ""


def say(text: str, emotion: str, speed: float) -> Action:
    """Speaks text. Emotion and speed are hints to the TTS; speed is 0.6 to 1.5."""
    return {"type": "say", "text": text, "generation_config": {"emotion": emotion, "speed": speed}}


def wait(seconds: float) -> Action:
    """Says nothing for a while and lets the caller finish or think."""
    return {"type": "wait", "seconds": seconds}


def transfer(to: str, reason: str) -> Action:
    """Hands the call to a human queue. The queue name is yours to choose."""
    return {"type": "transfer", "to": to, "reason": reason}


def end_call(reason: str) -> Action:
    """Hangs up."""
    return {"type": "end_call", "reason": reason}


def load_json(path: str | Path) -> Transcript:
    """Reads a transcript from a JSON file, for your own tests. The fixtures
    carry their transcripts inline in YAML with the same keys."""
    try:
        return Transcript.from_dict(json.loads(Path(path).read_text()))
    except json.JSONDecodeError as e:
        raise ValueError(f"{path}: {e}") from e
