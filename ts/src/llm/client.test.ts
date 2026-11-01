import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { test } from "node:test";

import { MODEL, OpenAI } from "./client.ts";

/** serve starts a local HTTP server that answers every request with body. */
function serve(body: string, onRequest: (headers: Record<string, unknown>, body: string) => void): Promise<Server> {
  const srv = createServer((req, res) => {
    let raw = "";
    req.on("data", (chunk) => (raw += chunk));
    req.on("end", () => {
      onRequest(req.headers, raw);
      res.setHeader("Content-Type", "application/json");
      res.end(body);
    });
  });
  return new Promise((resolve) => srv.listen(0, "127.0.0.1", () => resolve(srv)));
}

function baseUrl(srv: Server): string {
  const addr = srv.address();
  if (!addr || typeof addr === "string") throw new Error("no port");
  return `http://127.0.0.1:${addr.port}`;
}

// The request on the wire has the chat completions shape: tools wrapped as
// functions, assistant tool calls with null content, tool results by id,
// and structured output as response_format.json_schema.
test("OpenAI wire shape", async () => {
  let got: Record<string, any> = {};
  let auth: unknown;
  const srv = await serve(
    `{"choices":[{"message":{"content":"{\\"ok\\":true}","tool_calls":[{"id":"c1","type":"function","function":{"name":"book","arguments":"{}"}}]}}],"usage":{"prompt_tokens":12,"completion_tokens":3}}`,
    (headers, body) => {
      auth = headers.authorization;
      got = JSON.parse(body);
    },
  );
  try {
    const c = new OpenAI({ baseUrl: baseUrl(srv), apiKey: "k" });
    const resp = await c.chat({
      messages: [
        { role: "system", content: "sys" },
        { role: "assistant", tool_calls: [{ id: "t1", name: "book", arguments: `{"a":1}` }] },
        { role: "tool", tool_call_id: "t1", content: `{"ok":true}` },
      ],
      tools: [{ name: "book", description: "d", parameters: { type: "object" } }],
      response_format: { name: "verdict", schema: { type: "object" }, strict: true },
    });

    assert.equal(auth, "Bearer k");
    assert.equal(got.model, MODEL);
    const [, assistant, tool] = got.messages;
    assert.equal(assistant.content, null);
    assert.equal(assistant.tool_calls[0].type, "function");
    assert.equal(tool.tool_call_id, "t1");
    assert.equal(tool.content, `{"ok":true}`);
    assert.equal(got.tools[0].type, "function");
    assert.equal(got.tools[0].function.name, "book");
    assert.equal(got.response_format.type, "json_schema");
    assert.equal(got.response_format.json_schema.name, "verdict");
    assert.equal(got.response_format.json_schema.strict, true);

    assert.equal(resp.content, `{"ok":true}`);
    assert.equal(resp.tool_calls?.length, 1);
    assert.equal(resp.tool_calls?.[0]?.name, "book");
    assert.deepEqual(resp.usage, { prompt_tokens: 12, completion_tokens: 3 });
  } finally {
    srv.close();
  }
});
