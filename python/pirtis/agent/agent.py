"""The receptionist. This is where the work happens.

decide gets the call so far and returns what to do next. handlers, in
actions.py, carry those actions out. Everything else in the repo is plumbing
you can use or ignore.
"""

import json

from pirtis.api.client import Client as ApiClient
from pirtis.api.types import AvailabilityQuery, BookingRequest, Site
from pirtis.jsonutil import json_default
from pirtis.llm.client import Client as LlmClient
from pirtis.llm.client import Message
from pirtis.llm.loop import Tool, ToolResult, run_agent
from pirtis.transcript.types import Action, Transcript, say


def decide(t: Transcript, api: ApiClient, llm: LlmClient) -> list[Action]:
    """Returns the receptionist's next actions given the transcript so far.

    What is here now is a starting point. It passes the happy-path fixture
    and not much else.
    """
    units = api.list_units()
    system = (
        "You are the receptionist for Pirtis on Wheels, a mobile sauna rental. "
        f"Help the caller book a sauna. Today is {t.today}. "
        f"Units: {json.dumps(units, default=json_default, ensure_ascii=False)}"
    )

    # every item becomes history, whether the caller finished the sentence or not
    history: list[Message] = []
    for it in t.items:
        match it.type:
            case "message":
                history.append(Message(role=it.role or "user", content=it.content))
            case "function_call":
                history.append(
                    Message(role="assistant", content=f"[called {it.name} with {it.arguments}]")
                )
            case "function_call_output":
                history.append(Message(role="assistant", content=f"[result: {it.output}]"))

    # no people, no weight, no power. every unit comes back and the model picks.
    def availability(args: dict) -> ToolResult:
        q = AvailabilityQuery(from_=args["from"], to=args["to"])
        return ToolResult(data=api.availability(q))

    def book(args: dict) -> ToolResult:
        req = BookingRequest(
            customer_id=args["customer_id"],
            unit_id=args["unit_id"],
            start=args["start"],
            end=args["end"],
            people=args["people"],
            site=Site(power_available=True),  # nobody asked
            tow_capacity_kg=3500,  # nobody asked
        )
        return ToolResult(data=api.create_booking(req))

    tools = [
        Tool(
            name="availability",
            description="Free dates per unit between two dates, YYYY-MM-DD.",
            parameters={
                "type": "object",
                "properties": {"from": {"type": "string"}, "to": {"type": "string"}},
                "required": ["from", "to"],
            },
            execute=availability,
        ),
        Tool(
            name="book",
            description="Create a booking.",
            parameters={
                "type": "object",
                "properties": {
                    "customer_id": {"type": "string"},
                    "unit_id": {"type": "string"},
                    "start": {"type": "string"},
                    "end": {"type": "string"},
                    "people": {"type": "integer"},
                },
                "required": ["customer_id", "unit_id", "start", "end", "people"],
            },
            execute=book,
        ),
    ]

    res = run_agent(llm, system, history, tools, 8)
    if res.terminal is not None:
        return res.terminal
    return [say(res.text, "neutral", 1.0)]
