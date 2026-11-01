from pirtis.transcript.types import Item, Transcript


def last_message(t: Transcript) -> Item | None:
    """The last spoken line, skipping API call items."""
    for it in reversed(t.items):
        if it.type == "message":
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
