import json
from dataclasses import dataclass
from typing import Any

from pirtis.llm.client import Client, Message, Request, ResponseFormat
from pirtis.transcript.types import Effect, Transcript, action_type


@dataclass
class Verdict:
    """The judge's answer on one criterion."""

    criterion: str
    passed: bool
    reason: str


JUDGE_SYSTEM = """You grade one reply from a phone receptionist at a mobile sauna rental company.
You see the call so far and the receptionist's latest reply. Each criterion is a statement about that reply.
Mark it pass if the statement is true of the reply and fail if it is not, with a one-sentence reason.
Judge only the latest reply, using the call for context. Be strict about the substance and lenient about wording."""  # noqa: E501


def verdict_schema(criteria: list[str]) -> dict[str, Any]:
    """The judge's answer shape for these criteria: one required property per
    criterion, named criterion_1, criterion_2, ..., each carrying the
    criterion text as its description. Strict mode then guarantees exactly
    one verdict per criterion. Inside a verdict, reason comes before pass so
    the model writes its reasoning before it commits."""
    return {
        "type": "object",
        "properties": {
            _criterion_key(i): {
                "type": "object",
                "description": cr,
                "properties": {
                    "reason": {"type": "string", "description": "one sentence"},
                    "pass": {"type": "boolean"},
                },
                "required": ["reason", "pass"],
                "additionalProperties": False,
            }
            for i, cr in enumerate(criteria)
        },
        "required": [_criterion_key(i) for i in range(len(criteria))],
        "additionalProperties": False,
    }


def _criterion_key(i: int) -> str:
    return f"criterion_{i + 1}"


def judge(
    client: Client, tr: Transcript, effects: list[Effect], criteria: list[str]
) -> list[Verdict]:
    """Asks the model whether the last reply meets each criterion."""
    lines = ["Call so far:"]
    for it in tr.items:
        match it.type:
            case "message":
                who = "receptionist" if it.role == "assistant" else "caller"
                lines.append(f"{who}: {it.content}")
            case "function_call":
                lines.append(f"[receptionist's system called {it.name} with {it.arguments}]")
            case "function_call_output":
                lines.append(f"[result: {it.output}]")
    lines.append("\nReceptionist's latest reply:")
    said = []
    for e in effects:
        match action_type(e):
            case "speech":
                said.append(str(e.get("text")))
            case "silence":
                said.append("(says nothing, waits)")
            case "transfer":
                said.append("(transfers the call to a person)")
            case "hangup":
                said.append("(hangs up)")
    lines.append(" ".join(said) or "(nothing)")
    lines.append("\nCriteria:")
    for i, cr in enumerate(criteria):
        lines.append(f"{i + 1}. {cr}")

    resp = client.chat(
        Request(
            messages=[
                Message(role="system", content=JUDGE_SYSTEM),
                Message(role="user", content="\n".join(lines) + "\n"),
            ],
            temperature=0.0,
            response_format=ResponseFormat(
                name="verdict", schema=verdict_schema(criteria), strict=True
            ),
        )
    )

    try:
        parsed = json.loads(resp.content)
        if not isinstance(parsed, dict):
            raise ValueError("not an object")
    except ValueError as e:
        raise ValueError(f"unreadable verdict: {_truncate(resp.content, 200)}") from e
    out = []
    for i, cr in enumerate(criteria):
        v = parsed.get(_criterion_key(i))
        if not isinstance(v, dict):
            out.append(Verdict(criterion=cr, passed=False, reason="no verdict given"))
            continue
        out.append(
            Verdict(criterion=cr, passed=bool(v.get("pass")), reason=str(v.get("reason", "")))
        )
    return out


def _truncate(s: str, n: int) -> str:
    return s if len(s) <= n else s[:n] + "..."
