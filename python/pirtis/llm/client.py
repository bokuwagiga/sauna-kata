"""A non-streaming chat client in the OpenAI message format, plus a small tool
loop in loop.py. A Recorder counts calls and tokens, and writes every request
and response to disk when the harness is run with --trace.
"""

import json
import os
import time
from dataclasses import dataclass, field, replace
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Protocol
from urllib.error import HTTPError, URLError
from urllib.request import Request as HttpRequest
from urllib.request import urlopen

from pirtis.jsonutil import json_default


@dataclass
class ToolCall:
    """The model asking for a tool to run."""

    id: str
    name: str
    arguments: str  # JSON text


@dataclass
class Message:
    """One chat message. The system prompt is a Message with role "system",
    by convention the first one."""

    role: str  # system | user | assistant | tool
    content: str = ""  # text
    tool_calls: list[ToolCall] = field(default_factory=list)  # assistant only
    tool_call_id: str = ""  # tool only


@dataclass
class ToolSchema:
    """Tells the model what a tool is and what it takes."""

    name: str
    description: str = ""
    parameters: dict[str, Any] | None = None  # JSON schema object


@dataclass
class ResponseFormat:
    """Asks for structured output: the reply is JSON matching schema. schema
    is a JSON Schema object. With strict the model is held to it exactly,
    which requires additionalProperties: false and every property listed as
    required."""

    name: str
    schema: dict[str, Any]
    description: str = ""
    strict: bool = False


@dataclass
class Request:
    """One chat completion request."""

    messages: list[Message]
    tools: list[ToolSchema] = field(default_factory=list)
    temperature: float | None = None
    response_format: ResponseFormat | None = None


@dataclass
class Usage:
    """What the call cost in tokens."""

    prompt_tokens: int = 0
    completion_tokens: int = 0


@dataclass
class Response:
    """What came back: text, tool calls, or both."""

    content: str = ""
    tool_calls: list[ToolCall] = field(default_factory=list)
    usage: Usage = field(default_factory=Usage)


class Client(Protocol):
    """Anything that can answer a chat request."""

    def chat(self, req: Request) -> Response: ...


# The one model the kata runs on. Everyone is graded against the same model,
# so it is not configurable.
MODEL = "gpt-4o-mini"


class RetryableError(Exception):
    """A 429, a 5xx, or a failed connection."""

    def __init__(self, msg: str, after: float = 0.0):
        super().__init__(msg)
        self.after = after  # seconds, from Retry-After; 0 when unknown


class OpenAI:
    """Talks to an OpenAI-compatible chat completions endpoint."""

    def __init__(
        self, api_key: str, base_url: str = "https://api.openai.com/v1", timeout: float = 60.0
    ):
        self.api_key = api_key
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout

    def chat(self, req: Request) -> Response:
        """Rate limits, server errors and network errors are retried three
        times with a short backoff, honouring Retry-After."""
        for attempt in range(3):
            try:
                return self._chat_once(req)
            except RetryableError as e:
                if attempt == 2:
                    raise
                time.sleep(e.after or 2 ** (attempt + 1))
        raise AssertionError("unreachable")

    def _chat_once(self, req: Request) -> Response:
        body = json.dumps(_to_wire(req), ensure_ascii=False).encode()
        http_req = HttpRequest(
            self.base_url + "/chat/completions",
            data=body,
            method="POST",
            headers={
                "Content-Type": "application/json",
                "Authorization": "Bearer " + self.api_key,
                "api-key": self.api_key,  # Azure OpenAI reads this one
            },
        )
        try:
            with urlopen(http_req, timeout=self.timeout) as resp:
                raw = resp.read()
        except HTTPError as e:
            raw = e.read()
            if e.code == 429 or e.code >= 500:
                after = 0
                try:
                    after = int(e.headers.get("Retry-After", ""))
                except ValueError:
                    pass
                raise RetryableError(f"llm: status {e.code}: {_truncate(raw, 300)}", after) from e
            raise RuntimeError(f"llm: status {e.code}: {_error_message(raw)}") from e
        except (URLError, TimeoutError, OSError) as e:
            raise RetryableError(f"llm: {e}") from e
        try:
            w = json.loads(raw)
        except json.JSONDecodeError as e:
            raise RuntimeError(f"llm: unreadable body: {_truncate(raw, 300)}") from e
        if isinstance(w, dict) and w.get("error"):
            raise RuntimeError(f"llm: status 200: {_error_message(raw)}")
        return _from_wire(w)


def open_ai_from_env() -> OpenAI:
    """Reads LLM_API_KEY (required) and LLM_BASE_URL (optional)."""
    key = os.environ.get("LLM_API_KEY", "")
    if not key:
        raise RuntimeError("LLM_API_KEY is not set")
    base = os.environ.get("LLM_BASE_URL") or "https://api.openai.com/v1"
    return OpenAI(api_key=key, base_url=base)


def _to_wire(req: Request) -> dict[str, Any]:
    """The chat completions request shape."""
    messages = []
    for m in req.messages:
        wm: dict[str, Any] = {"role": m.role}
        # an assistant message that only carries tool calls has null content
        wm["content"] = m.content if m.content or not m.tool_calls else None
        if m.tool_calls:
            wm["tool_calls"] = [
                {
                    "id": tc.id,
                    "type": "function",
                    "function": {"name": tc.name, "arguments": tc.arguments},
                }
                for tc in m.tool_calls
            ]
        if m.tool_call_id:
            wm["tool_call_id"] = m.tool_call_id
        messages.append(wm)
    wr: dict[str, Any] = {"model": MODEL, "messages": messages}
    if req.tools:
        wr["tools"] = [
            {
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters or {"type": "object", "properties": {}},
                },
            }
            for t in req.tools
        ]
    if req.temperature is not None:
        wr["temperature"] = req.temperature
    if req.response_format is not None:
        rf = req.response_format
        js: dict[str, Any] = {"name": rf.name, "schema": rf.schema}
        if rf.description:
            js["description"] = rf.description
        if rf.strict:
            js["strict"] = True
        wr["response_format"] = {"type": "json_schema", "json_schema": js}
    return wr


def _from_wire(w: Any) -> Response:
    choices = w.get("choices") if isinstance(w, dict) else None
    if not choices:
        raise RuntimeError("llm: no choices in response")
    msg = choices[0].get("message") or {}
    usage = w.get("usage") or {}
    return Response(
        content=msg.get("content") or "",
        tool_calls=[
            ToolCall(
                id=tc["id"], name=tc["function"]["name"], arguments=tc["function"]["arguments"]
            )
            for tc in msg.get("tool_calls") or []
        ],
        usage=Usage(
            prompt_tokens=usage.get("prompt_tokens", 0),
            completion_tokens=usage.get("completion_tokens", 0),
        ),
    )


def _error_message(raw: bytes) -> str:
    try:
        return json.loads(raw)["error"]["message"]
    except (ValueError, KeyError, TypeError):
        return _truncate(raw, 300)


def _truncate(raw: bytes | str, n: int) -> str:
    s = raw.decode(errors="replace") if isinstance(raw, bytes) else raw
    return s if len(s) <= n else s[:n] + "..."


@dataclass
class Stats:
    """What a Recorder has seen so far."""

    calls: int = 0
    prompt_tokens: int = 0
    completion_tokens: int = 0
    duration_ms: int = 0
    errors: int = 0


class Recorder:
    """Wraps a Client, counts calls and tokens, and writes every request and
    response to a directory, one JSON file per call. Without a directory
    nothing is written, only counted."""

    def __init__(self, inner: Client, dir: Path | None = None):
        self._inner = inner
        self._dir = dir
        self._n = 0
        self._stats = Stats()

    def stats(self) -> Stats:
        """The totals so far."""
        return replace(self._stats)

    def chat(self, req: Request) -> Response:
        at = datetime.now(UTC)
        start = time.perf_counter()
        try:
            resp = self._inner.chat(req)
        except Exception as e:
            self._count(start, None)
            self._write({"error": str(e)}, at, start, req)
            raise
        self._count(start, resp)
        self._write({"response": resp}, at, start, req)
        return resp

    def _count(self, start: float, resp: Response | None) -> None:
        s = self._stats
        self._n += 1
        s.calls += 1
        s.duration_ms += round((time.perf_counter() - start) * 1000)
        if resp is None:
            s.errors += 1
        else:
            s.prompt_tokens += resp.usage.prompt_tokens
            s.completion_tokens += resp.usage.completion_tokens

    def _write(self, outcome: dict[str, Any], at: datetime, start: float, req: Request) -> None:
        if self._dir is None:
            return
        rec = {
            "at": at.isoformat(),
            "duration_ms": round((time.perf_counter() - start) * 1000),
            "request": req,
            **outcome,
        }
        try:
            self._dir.mkdir(parents=True, exist_ok=True)
            (self._dir / f"{self._n:03d}.json").write_text(
                json.dumps(rec, indent=2, default=json_default, ensure_ascii=False)
            )
        except OSError:
            pass
