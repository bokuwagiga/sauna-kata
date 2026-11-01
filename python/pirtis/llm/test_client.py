import json
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

import pytest

from pirtis.llm.client import MODEL, Message, OpenAI, Request, ResponseFormat, ToolCall, ToolSchema

CANNED = (
    b'{"choices":[{"message":{"content":"{\\"ok\\":true}","tool_calls":[{"id":"c1","type":"function",'
    b'"function":{"name":"book","arguments":"{}"}}]}}],"usage":{"prompt_tokens":12,"completion_tokens":3}}'
)


class _Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        self.server.seen.append((dict(self.headers), json.loads(body)))  # type: ignore[attr-defined]
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(CANNED)

    def log_message(self, *args):
        pass


@pytest.fixture
def server():
    srv = HTTPServer(("127.0.0.1", 0), _Handler)
    srv.seen = []  # type: ignore[attr-defined]
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield srv
    srv.shutdown()
    srv.server_close()


def test_openai_wire_shape(server):
    """The request on the wire has the chat completions shape: tools wrapped
    as functions, assistant tool calls with null content, tool results by
    id, and structured output as response_format.json_schema."""
    c = OpenAI(api_key="k", base_url=f"http://127.0.0.1:{server.server_address[1]}")
    resp = c.chat(
        Request(
            messages=[
                Message(role="system", content="sys"),
                Message(
                    role="assistant",
                    tool_calls=[ToolCall(id="t1", name="book", arguments='{"a":1}')],
                ),
                Message(role="tool", tool_call_id="t1", content='{"ok":true}'),
            ],
            tools=[ToolSchema(name="book", description="d", parameters={"type": "object"})],
            response_format=ResponseFormat(name="verdict", schema={"type": "object"}, strict=True),
        )
    )

    headers, got = server.seen[0]
    assert headers["Authorization"] == "Bearer k"
    assert got["model"] == MODEL
    msgs = got["messages"]
    assert msgs[1]["content"] is None
    assert msgs[1]["tool_calls"][0]["type"] == "function"
    assert msgs[2]["tool_call_id"] == "t1" and msgs[2]["content"] == '{"ok":true}'
    tool = got["tools"][0]
    assert tool["type"] == "function" and tool["function"]["name"] == "book"
    rf = got["response_format"]
    assert rf["type"] == "json_schema"
    assert rf["json_schema"]["name"] == "verdict" and rf["json_schema"]["strict"] is True

    assert resp.content == '{"ok":true}'
    assert len(resp.tool_calls) == 1 and resp.tool_calls[0].name == "book"
    assert (resp.usage.prompt_tokens, resp.usage.completion_tokens) == (12, 3)
