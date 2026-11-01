import { ApiError } from "../api/types.ts";
import type { Action } from "../transcript/types.ts";
import type { Client, Message, ToolSchema } from "./client.ts";

/**
 * ToolResult is what a tool hands back. Set data to feed something to the
 * model and keep going. Set terminal to stop the loop and make those actions
 * the answer.
 */
export interface ToolResult {
  data?: unknown;
  terminal?: Action[];
}

/** Tool is a schema the model sees plus the code that runs when it calls it. */
export interface Tool extends ToolSchema {
  /** args is the model's arguments, already parsed from JSON. */
  execute(args: unknown): ToolResult | Promise<ToolResult>;
}

/**
 * AgentResult is what runAgent returns: the model's text, or terminal
 * actions from a tool.
 */
export interface AgentResult {
  text: string;
  terminal?: Action[];
}

/**
 * MaxCallsError is thrown when the model is still asking for tools after
 * maxCalls rounds. text holds whatever it said along the way.
 */
export class MaxCallsError extends Error {
  readonly text: string;

  constructor(text: string) {
    super("llm: tool loop hit the call limit");
    this.name = "MaxCallsError";
    this.text = text;
  }
}

/**
 * runAgent runs the tool loop: call the model, run whatever tools it asks
 * for, feed the results back, repeat until it answers in text or a tool
 * returns terminal. maxCalls caps the number of model calls; 0 means 8, and
 * hitting the cap throws MaxCallsError.
 *
 * A tool the model names but that does not exist, and a tool that throws,
 * both go back to the model as JSON rather than stopping the loop.
 *
 * Text the model produces alongside a tool call ("one moment") is kept and
 * joined to what it says after, since on a phone that is one reply.
 */
export async function runAgent(
  client: Client,
  system: string,
  history: Message[],
  tools: Tool[],
  maxCalls = 8,
): Promise<AgentResult> {
  if (maxCalls <= 0) maxCalls = 8;
  const msgs: Message[] = [{ role: "system", content: system }, ...history];
  const byName = new Map(tools.map((t) => [t.name, t]));
  const schemas: ToolSchema[] = tools.map(({ execute: _, ...schema }) => schema);

  let text = "";
  for (let i = 0; i < maxCalls; i++) {
    const reply = await client.chat({ messages: msgs, tools: schemas });
    if (reply.content) text = `${text} ${reply.content}`.trim();
    if (!reply.tool_calls?.length) return { text };

    msgs.push({ role: "assistant", content: reply.content, tool_calls: reply.tool_calls });
    for (const call of reply.tool_calls) {
      const res = await runTool(byName.get(call.name), call.name, call.arguments);
      if (res.terminal !== undefined) return { text, terminal: res.terminal };
      msgs.push({ role: "tool", tool_call_id: call.id, content: JSON.stringify(res.data ?? null) });
    }
  }
  throw new MaxCallsError(text);
}

async function runTool(tool: Tool | undefined, name: string, args: string): Promise<ToolResult> {
  if (!tool) return { data: { error: `unknown tool ${name}` } };
  try {
    return await tool.execute(JSON.parse(args));
  } catch (err) {
    return { data: { error: errorCode(err), detail: err instanceof Error ? err.message : String(err) } };
  }
}

/** errorCode is the API reason code when err is an ApiError, else TOOL_FAILED. */
export function errorCode(err: unknown): string {
  return err instanceof ApiError ? err.code : "TOOL_FAILED";
}
