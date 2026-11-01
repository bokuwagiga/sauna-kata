from collections.abc import Callable
from dataclasses import dataclass

from pirtis.api.client import Client
from pirtis.transcript.types import Action, Effect, Transcript


@dataclass
class HandlerContext:
    """What a handler can reach while carrying out an action."""

    api: Client
    transcript: Transcript


# A Handler carries out one action and reports what happened as an Effect.
Handler = Callable[[Action, HandlerContext], Effect]


def _say(a: Action, _: HandlerContext) -> Effect:
    return {
        "type": "speech",
        "text": a.get("text"),
        "generation_config": a.get("generation_config"),
    }


def _wait(a: Action, _: HandlerContext) -> Effect:
    return {"type": "silence", "seconds": a.get("seconds")}


def _transfer(a: Action, _: HandlerContext) -> Effect:
    return {"type": "transfer", "to": a.get("to"), "reason": a.get("reason")}


def _end_call(a: Action, _: HandlerContext) -> Effect:
    return {"type": "hangup", "reason": a.get("reason")}


# handlers maps an action type to the code that carries it out. The harness
# runs every action decide returns through the handler for its type and
# asserts on the effects. An action with no handler fails the fixture.
#
# These four are the starting set. Add your own, change these, or wrap them.
handlers: dict[str, Handler] = {
    "say": _say,
    "wait": _wait,
    "transfer": _transfer,
    "end_call": _end_call,
}
