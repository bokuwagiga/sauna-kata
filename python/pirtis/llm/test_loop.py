from types import SimpleNamespace

import pytest

from pirtis.llm.client import Message, Request, Response, ToolCall
from pirtis.llm.fake import Fake
from pirtis.llm.loop import MaxCallsError, Tool, ToolResult, run_agent
from pirtis.transcript.types import transfer


def echo_tool(name: str) -> Tool:
    return Tool(
        name=name,
        description="echo",
        parameters={"type": "object"},
        execute=lambda args: ToolResult(data={"got": args}),
    )


def test_run_agent_tool_then_text():
    """The loop runs the tool the model asks for, feeds the result back, and
    returns the text the model then produces, joined with any text said
    alongside the call."""
    fake = Fake(
        [
            Response(
                content="One moment.",
                tool_calls=[ToolCall(id="t1", name="lookup", arguments='{"q":1}')],
            ),
            Response(content="Found it."),
        ]
    )
    res = run_agent(fake, "sys", [], [echo_tool("lookup")])
    assert res.text == "One moment. Found it."
    assert fake.i == 2


def test_run_agent_terminal():
    """A terminal tool result stops the loop and becomes the answer."""
    tool = Tool(
        name="transfer",
        parameters={"type": "object"},
        execute=lambda args: ToolResult(terminal=[transfer("front_desk", "asked")]),
    )
    fake = Fake([Response(tool_calls=[ToolCall(id="t1", name="transfer", arguments="{}")])])
    res = run_agent(fake, "sys", [], [tool])
    assert res.terminal is not None and len(res.terminal) == 1
    assert res.terminal[0]["type"] == "transfer"
    assert fake.i == 1, "loop must stop after the terminal tool"


def test_run_agent_tool_errors_go_back():
    """An unknown tool and a failing tool both go back to the model as JSON."""

    def boom(args):
        raise RuntimeError("boom")

    failing = Tool(name="book", parameters={"type": "object"}, execute=boom)
    seen: list[Message] = []
    fake = Fake(
        [Response(tool_calls=[ToolCall("t1", "nope", "{}"), ToolCall("t2", "book", "{}")])],
        default=Response(content="ok"),
    )

    def spy_chat(req: Request) -> Response:
        seen[:] = req.messages
        return fake.chat(req)

    run_agent(SimpleNamespace(chat=spy_chat), "sys", [], [failing])
    tool_msgs = [m.content for m in seen if m.role == "tool"]
    assert len(tool_msgs) == 2
    assert "unknown tool" in tool_msgs[0]
    assert "TOOL_FAILED" in tool_msgs[1]


def test_run_agent_max_calls():
    """A model that never stops calling tools hits the cap and the caller
    hears about it."""
    calls = 0

    def forever(req: Request) -> Response:
        nonlocal calls
        calls += 1
        return Response(tool_calls=[ToolCall("t", "lookup", "{}")])

    with pytest.raises(MaxCallsError):
        run_agent(SimpleNamespace(chat=forever), "sys", [], [echo_tool("lookup")], 3)
    assert calls == 3
