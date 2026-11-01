"""The receptionist. decide reads the call in plain code first, makes at most one
model call to understand what the caller said, then lets code check the rules
against the API and write the reply.
"""

from datetime import date, timedelta

from pirtis.agent.booking import Offer, find_offer, missing_question, weekend_end
from pirtis.agent.extract import CallerInfo, extract
from pirtis.agent.facts import (
    caller_emotion,
    caller_gone_quiet,
    caller_still_talking,
    identify_result,
    last_message,
)
from pirtis.agent.speech import booked_words, booking_words, no_offer_words, offer_words
from pirtis.api.client import Client as ApiClient
from pirtis.api.types import (
    STATUS_BOOKED,
    ApiError,
    AvailabilityQuery,
    Booking,
    BookingPatch,
    BookingQuery,
    BookingRequest,
    Site,
    Unit,
)
from pirtis.llm.client import Client as LlmClient
from pirtis.transcript.types import Action, Transcript, say, transfer, wait

_QUEUE = "front_desk"  # the human queue we hand calls to


def _say(text: str) -> list[Action]:
    return [say(text, "neutral", 1.0)]


def decide(t: Transcript, api: ApiClient, llm: LlmClient) -> list[Action]:
    """Returns the receptionist's next actions given the transcript so far."""
    # Turn-taking first: cheap, instant and certain, so no model call is needed.
    if caller_still_talking(t):
        return [wait(1.0)]
    if caller_gone_quiet(t):
        return [say("Are you still there? Take your time.", "warm", 1.0)]
    last = last_message(t)
    if last is not None and last.role == "assistant":
        return [wait(1.0)]
    # Tone is a noisy signal, so only a strong reading transfers on its own.
    # An explicit "can I talk to a person" comes from the model, as intent staff.
    if caller_emotion(t, "Anger") >= 0.7:
        return [transfer(_QUEUE, "caller is upset")]
    who = identify_result(t)
    if who is not None and not who.ok:
        return [transfer(_QUEUE, "caller is not a known customer")]

    units = api.list_units()
    info = extract(t, units, llm)
    if info.intent == "staff":
        return [transfer(_QUEUE, "caller asked for a person")]

    # Nothing about a customer is read or changed before they are identified.
    if who is None:
        # identify needs first and last name; a first name alone would fail it
        if info.name is None or len(info.name.split()) < 2:
            if info.intent in ("check", "change", "cancel"):
                return _say("I can look that up for you. Can I get your full name first?")
            return _say("Can I get your full name, please?")
        who = api.identify_customer(t.caller.phone, info.name)
        if not who.ok:
            return [transfer(_QUEUE, "caller is not a known customer")]

    match info.intent:
        case "book":
            return _book(t, api, units, info, who.customer_id)
        case "change":
            return _change(t, api, units, info, who.customer_id)
        case "check":
            return _check(t, api, units, who.customer_id)
        case "cancel":
            return [transfer(_QUEUE, "caller wants to cancel")]
        case _:
            return _say(info.answer or "Sorry, could you say that again?")


def _book(
    t: Transcript, api: ApiClient, units: list[Unit], info: CallerInfo, customer_id: str
) -> list[Action]:
    """Asks what is missing, offers what the rules allow, and books only what the
    caller agreed to."""
    question = missing_question(info)
    if question is not None:
        return _say(question)
    offer = find_offer(info, units, api, t.today)
    if not isinstance(offer, Offer):
        return _say(no_offer_words(offer, units))
    mine = _own_bookings(api, customer_id, offer.start, offer.end)
    if mine:
        return _say(f"You already have {booking_words(mine[0], units)}. Shall I move that one?")
    if info.confirmed and _agreed(t, offer):
        return _create(t, api, units, info, customer_id, offer)
    return _say(offer_words(offer, info))


def _agreed(t: Transcript, offer: Offer) -> bool:
    """The caller said yes to this exact offer: nothing was changed from what they
    asked, and our last line named this unit. The model's confirmed alone is not
    enough; it has said yes before we proposed anything."""
    ours = last_message(t, "assistant")
    return (
        ours is not None
        and offer.unit.name in ours.content
        and not offer.taken
        and not offer.weekend_extended
    )


def _create(
    t: Transcript,
    api: ApiClient,
    units: list[Unit],
    info: CallerInfo,
    customer_id: str,
    offer: Offer,
) -> list[Action]:
    """Books the offer. If someone took the dates between our check and the write
    (409), looks again once: the same offer is retried, anything else is offered."""
    req = BookingRequest(
        customer_id=customer_id,
        unit_id=offer.unit.id,
        start=offer.start,
        end=offer.end,
        people=info.people,
        site=Site(power_available=bool(info.power)),  # unknown only for wood units
        tow_capacity_kg=info.tow_kg,
    )
    try:
        api.create_booking(req)
    except ApiError as e:
        if e.status != 409:
            return [transfer(_QUEUE, f"booking failed: {e.code}")]
        again = find_offer(info, units, api, t.today)
        if not isinstance(again, Offer):
            return _say("Sorry, those dates just went. " + no_offer_words(again, units))
        if (again.unit.id, again.start, again.end) != (offer.unit.id, offer.start, offer.end):
            return _say("Sorry, those dates just went. " + offer_words(again, info))
        try:
            api.create_booking(req)
        except ApiError as e:
            return [transfer(_QUEUE, f"booking failed: {e.code}")]
    return _say(booked_words(offer))


def _own_bookings(api: ApiClient, customer_id: str, start: str, end: str) -> list[Booking]:
    """The caller's live bookings that overlap start to end. The API refuses a
    second booking for the same customer on the same days."""
    q = BookingQuery(customer_id=customer_id, from_=start, to=end)
    return [b for b in api.list_bookings(q) if b.status == STATUS_BOOKED]


def _upcoming(t: Transcript, api: ApiClient, customer_id: str) -> list[Booking]:
    """The caller's live bookings that have not ended, soonest first."""
    q = BookingQuery(customer_id=customer_id, from_=t.today)
    live = (b for b in api.list_bookings(q) if b.status == STATUS_BOOKED)
    return sorted(live, key=lambda b: b.start)


def _check(t: Transcript, api: ApiClient, units: list[Unit], customer_id: str) -> list[Action]:
    """Tells an identified caller what they have booked."""
    mine = _upcoming(t, api, customer_id)
    if not mine:
        return _say("I can't see a booking under your name. Would you like to make one?")
    return _say(f"Yes, you have {booking_words(mine[0], units)}.")


def _change(
    t: Transcript, api: ApiClient, units: list[Unit], info: CallerInfo, customer_id: str
) -> list[Action]:
    """Moves the caller's next booking to the dates they asked for. An explicit
    "move it" is the agreement, so this acts in the same turn."""
    mine = _upcoming(t, api, customer_id)
    if not mine:
        return _say("I can't see a booking under your name. Would you like to make one?")
    if info.start is None:
        return _say("Which dates would you like to move it to?")
    b = mine[0]
    start, end = info.start, weekend_end(info.start, info.end)
    if not _free_for_move(api, b, start, end):
        return _say("Sorry, that sauna isn't free then. Would other dates work?")
    others = [o for o in _own_bookings(api, customer_id, start, end) if o.id != b.id]
    if others:
        return _say(f"You already have {booking_words(others[0], units)} then.")
    moved = api.update_booking(b.id, b.version, BookingPatch(start=start, end=end))
    return _say(f"Done, I've moved it. That's {booking_words(moved, units)}.")


def _free_for_move(api: ApiClient, b: Booking, start: str, end: str) -> bool:
    """The new dates are free for the booking's unit. Availability counts the
    booking itself as busy, so a move that stays within a day of its old dates
    cannot be checked this way; it is refused rather than risk a failed write."""
    if start <= _shift(b.end, 1) and _shift(b.start, -1) <= end:
        return False
    for ua in api.availability(AvailabilityQuery(from_=start, to=end)):
        if ua.unit_id == b.unit_id:
            return any(r.start <= start and end <= r.end for r in ua.free)
    return False


def _shift(iso: str, days: int) -> str:
    """A YYYY-MM-DD date moved by a number of days."""
    return (date.fromisoformat(iso) + timedelta(days=days)).isoformat()
