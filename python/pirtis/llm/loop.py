import json
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from pirtis.api.types import ApiError
from pirtis.jsonutil import json_default
from pirtis.llm.client import Client, Message, Request, ToolSchema
from pirtis.transcript.types import Action


@dataclass
class ToolResult:
    """What a tool hands back. Set data to feed something to the model and
    keep going. Set terminal to stop the loop and make those actions the
    answer."""

    data: Any = None
    terminal: list[Action] | None = None


@dataclass(kw_only=True)
class Tool(ToolSchema):
    """A schema the model sees plus the code that runs when it calls it.
    execute gets the parsed JSON arguments."""

    execute: Callable[[Any], ToolResult]


@dataclass
class AgentResult:
    """What run_agent returns: the model's text, or terminal actions from a
    tool."""

    text: str = ""
    terminal: list[Action] | None = field(default=None)


class MaxCallsError(Exception):
    """The model was still asking for tools after max_calls rounds. text
    holds whatever it said along the way."""

    def __init__(self, text: str):
        super().__init__("llm: tool loop hit the call limit")
        self.text = text


def run_agent(
    client: Client, system: str, history: list[Message], tools: list[Tool], max_calls: int = 8
) -> AgentResult:
    """Runs the tool loop: call the model, run whatever tools it asks for,
    feed the results back, repeat until it answers in text or a tool returns
    terminal. max_calls caps the number of model calls; 0 means 8, and
    hitting the cap raises MaxCallsError.

    A tool the model names but that does not exist, and a tool that raises,
    both go back to the model as JSON rather than stopping the loop.

    Text the model produces alongside a tool call ("one moment") is kept and
    joined to what it says after, since on a phone that is one reply.
    """
    if max_calls <= 0:
        max_calls = 8
    msgs = [Message(role="system", content=system), *history]
    by_name = {t.name: t for t in tools}
    schemas = [ToolSchema(t.name, t.description, t.parameters) for t in tools]

    text = ""
    for _ in range(max_calls):
        reply = client.chat(Request(messages=msgs, tools=schemas))
        if reply.content:
            text = (text + " " + reply.content).strip()
        if not reply.tool_calls:
            return AgentResult(text=text)

        msgs.append(Message(role="assistant", content=reply.content, tool_calls=reply.tool_calls))
        for call in reply.tool_calls:
            tool = by_name.get(call.name)
            if tool is None:
                res = ToolResult(data={"error": "unknown tool " + call.name})
            else:
                try:
                    res = tool.execute(json.loads(call.arguments))
                except Exception as e:
                    res = ToolResult(data={"error": error_code(e), "detail": str(e)})
            if res.terminal is not None:
                return AgentResult(text=text, terminal=res.terminal)
            content = json.dumps(res.data, default=json_default, ensure_ascii=False)
            msgs.append(Message(role="tool", tool_call_id=call.id, content=content))
    raise MaxCallsError(text)


def error_code(err: BaseException) -> str:
    """The API reason code when err is an ApiError, else TOOL_FAILED."""
    return err.code if isinstance(err, ApiError) else "TOOL_FAILED"
