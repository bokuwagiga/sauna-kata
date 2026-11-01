import json

from pirtis.api.types import IdentifyResult
from pirtis.transcript.types import Item, Transcript


def last_message(t: Transcript, who: str = "any") -> Item | None:
    """The last spoken line, by anyone or by role who, skipping API call items."""
    for it in reversed(t.items):
        if it.type == "message" and (who == "any" or it.role == who):
            return it
    return None


def caller_still_talking(t: Transcript) -> bool:
    """The caller's last line is still changing, or they haven't paused yet."""
    last = last_message(t)
    return last is not None and last.role == "user" and not (last.is_final and last.speech_final)


def caller_gone_quiet(t: Transcript, after: float = 5.0) -> bool:
    """We spoke last and the caller has said nothing for `after` seconds."""
    last = last_message(t)
    return last is not None and last.role == "assistant" and t.now - last.end > after


def caller_emotion(t: Transcript, name: str) -> float:
    """The score of emotion `name` on the caller's last line, 0.0 if absent."""
    last = last_message(t, "user")
    if last is None or last.prosody is None:
        return 0.0
    for e in last.prosody.emotions:
        if e.name == name:
            return e.score
    return 0.0


def identify_result(t: Transcript) -> IdentifyResult | None:
    """The outcome of the last customers.identify in the call, or None if there was none."""
    for it in reversed(t.items):
        if it.type == "function_call" and it.name == "customers.identify":
            call_id = it.call_id
            break
    else:
        return None
    for it in reversed(t.items):
        if it.type == "function_call_output" and call_id == it.call_id:
            d = json.loads(it.output)
            break
    else:
        return None
    return IdentifyResult(ok=bool(d.get("ok")), customer_id=d.get("customer_id", ""))
