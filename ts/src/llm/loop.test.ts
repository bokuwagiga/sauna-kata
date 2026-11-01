import assert from "node:assert/strict";
import { test } from "node:test";

import { transfer } from "../transcript/types.ts";
import type { Client, Message, Request, Response } from "./client.ts";
import { Fake, fake } from "./fake.ts";
import { MaxCallsError, runAgent, type Tool } from "./loop.ts";

function echoTool(name: string): Tool {
  return {
    name,
    description: "echo",
    parameters: { type: "object" },
    execute: (args) => ({ data: { got: args } }),
  };
}

function clientFunc(fn: (req: Request) => Promise<Response>): Client {
  return { chat: fn };
}

// The loop runs the tool the model asks for, feeds the result back, and
// returns the text the model then produces, joined with any text said
// alongside the call.
test("runAgent: tool then text", async () => {
  const model = fake(
    { content: "One moment.", tool_calls: [{ id: "t1", name: "lookup", arguments: `{"q":1}` }] },
    { content: "Found it." },
  );
  const res = await runAgent(model, "sys", [], [echoTool("lookup")]);
  assert.equal(res.text, "One moment. Found it.");
  assert.equal(model.calls, 2, "want 2 model calls");
});

// A terminal tool result stops the loop and becomes the answer.
test("runAgent: terminal", async () => {
  const transferTool: Tool = {
    name: "transfer",
    parameters: { type: "object" },
    execute: () => ({ terminal: [transfer("front_desk", "asked")] }),
  };
  const model = fake({ tool_calls: [{ id: "t1", name: "transfer", arguments: "{}" }] });
  const res = await runAgent(model, "sys", [], [transferTool]);
  assert.equal(res.terminal?.length, 1);
  assert.equal(res.terminal?.[0]?.type, "transfer");
  assert.equal(model.calls, 1, "loop must stop after the terminal tool");
});

// An unknown tool and a failing tool both go back to the model as JSON.
test("runAgent: tool errors go back", async () => {
  const failing: Tool = {
    name: "book",
    parameters: { type: "object" },
    execute: () => {
      throw new Error("boom");
    },
  };
  let seen: Message[] = [];
  const model = new Fake(
    [
      {
        tool_calls: [
          { id: "t1", name: "nope", arguments: "{}" },
          { id: "t2", name: "book", arguments: "{}" },
        ],
      },
    ],
    { content: "ok" },
  );
  const spy = clientFunc((req) => {
    seen = req.messages;
    return model.chat(req);
  });
  await runAgent(spy, "sys", [], [failing]);
  const toolMsgs = seen.filter((m) => m.role === "tool").map((m) => m.content ?? "");
  assert.equal(toolMsgs.length, 2);
  assert.match(toolMsgs[0]!, /unknown tool/);
  assert.match(toolMsgs[1]!, /TOOL_FAILED/);
});

// A model that never stops calling tools hits the cap and the caller hears
// about it.
test("runAgent: max calls", async () => {
  let calls = 0;
  const forever = clientFunc(async () => {
    calls++;
    return { tool_calls: [{ id: "t", name: "lookup", arguments: "{}" }] };
  });
  await assert.rejects(runAgent(forever, "sys", [], [echoTool("lookup")], 3), MaxCallsError);
  assert.equal(calls, 3, "want exactly 3 calls");
});
