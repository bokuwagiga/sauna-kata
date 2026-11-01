// The llm module is a non-streaming chat client in the OpenAI message format,
// plus a small tool loop. A Recorder counts calls and tokens, and writes every
// request and response to disk when the harness is run with --trace.

import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

/**
 * Message is one chat message. The system prompt is a Message with role
 * "system", by convention the first one.
 */
export interface Message {
  role: "system" | "user" | "assistant" | "tool";
  /** text */
  content?: string;
  /** assistant only */
  tool_calls?: ToolCall[];
  /** tool only */
  tool_call_id?: string;
}

/** ToolCall is the model asking for a tool to run. */
export interface ToolCall {
  id: string;
  name: string;
  /** JSON text */
  arguments: string;
}

/** ToolSchema tells the model what a tool is and what it takes. */
export interface ToolSchema {
  name: string;
  description?: string;
  /** JSON schema object */
  parameters?: Record<string, unknown>;
}

/**
 * ResponseFormat asks for structured output: the reply is JSON matching
 * schema. schema is a JSON Schema object. With strict the model is held to
 * it exactly, which requires additionalProperties: false and every property
 * listed as required.
 */
export interface ResponseFormat {
  name: string;
  description?: string;
  schema: Record<string, unknown>;
  strict?: boolean;
}

/** Request is one chat completion request. */
export interface Request {
  messages: Message[];
  tools?: ToolSchema[];
  temperature?: number;
  response_format?: ResponseFormat;
}

/** Usage is what the call cost in tokens. */
export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
}

/** Response is what came back: text, tool calls, or both. */
export interface Response {
  content?: string;
  tool_calls?: ToolCall[];
  usage?: Usage;
}

/** Client is anything that can answer a chat request. */
export interface Client {
  chat(req: Request): Promise<Response>;
}

/**
 * MODEL is the one model the kata runs on. Everyone is graded against the
 * same model, so it is not configurable.
 */
export const MODEL = "gpt-4o-mini";

export interface OpenAIOptions {
  apiKey: string;
  /** defaults to https://api.openai.com/v1 */
  baseUrl?: string;
  /** per-request timeout; defaults to 60 seconds */
  timeoutMs?: number;
  /** defaults to the global fetch; tests inject their own */
  fetch?: typeof fetch;
}

/** OpenAI talks to an OpenAI-compatible chat completions endpoint. */
export class OpenAI implements Client {
  readonly baseUrl: string;
  readonly apiKey: string;
  readonly timeoutMs: number;
  private readonly fetch: typeof fetch;

  constructor(opts: OpenAIOptions) {
    let baseUrl = opts.baseUrl ?? "https://api.openai.com/v1";
    while (baseUrl.endsWith("/")) baseUrl = baseUrl.slice(0, -1);
    this.baseUrl = baseUrl;
    this.apiKey = opts.apiKey;
    this.timeoutMs = opts.timeoutMs ?? 60_000;
    this.fetch = opts.fetch ?? globalThis.fetch;
  }

  /**
   * Rate limits, server errors and network errors are retried three times
   * with a short backoff, honouring Retry-After.
   */
  async chat(req: Request): Promise<Response> {
    let last: RetryableError | undefined;
    for (let attempt = 0; attempt < 3; attempt++) {
      if (attempt > 0) {
        await sleep(last?.afterMs || 2 ** attempt * 1000);
      }
      try {
        return await this.chatOnce(req);
      } catch (err) {
        if (!(err instanceof RetryableError)) throw err;
        last = err;
      }
    }
    throw last;
  }

  private async chatOnce(req: Request): Promise<Response> {
    const body: WireRequest = {
      model: MODEL,
      messages: req.messages.map(toWireMessage),
      temperature: req.temperature,
    };
    if (req.tools?.length) {
      body.tools = req.tools.map((t) => ({
        type: "function",
        function: {
          name: t.name,
          description: t.description ?? "",
          parameters: t.parameters ?? { type: "object", properties: {} },
        },
      }));
    }
    if (req.response_format) {
      body.response_format = { type: "json_schema", json_schema: req.response_format };
    }

    let res: globalThis.Response;
    let raw: string;
    try {
      res = await this.fetch(`${this.baseUrl}/chat/completions`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${this.apiKey}`,
          "api-key": this.apiKey, // Azure OpenAI reads this one
        },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(this.timeoutMs),
      });
      raw = await res.text();
    } catch (err) {
      throw new RetryableError(err instanceof Error ? err.message : String(err), { cause: err });
    }
    if (res.status === 429 || res.status >= 500) {
      const after = Number(res.headers.get("Retry-After"));
      throw new RetryableError(`llm: status ${res.status}: ${truncate(raw, 300)}`, {
        afterMs: after > 0 ? after * 1000 : 0,
      });
    }
    let w: WireResponse;
    try {
      w = JSON.parse(raw) as WireResponse;
    } catch {
      throw new Error(`llm: status ${res.status}, unreadable body: ${truncate(raw, 300)}`);
    }
    if (res.status !== 200 || w.error) {
      throw new Error(`llm: status ${res.status}: ${w.error?.message ?? truncate(raw, 300)}`);
    }
    const choice = w.choices?.[0];
    if (!choice) throw new Error("llm: no choices in response");
    const out: Response = { usage: w.usage };
    if (choice.message.content != null) out.content = choice.message.content;
    if (choice.message.tool_calls?.length) {
      out.tool_calls = choice.message.tool_calls.map((tc) => ({
        id: tc.id,
        name: tc.function.name,
        arguments: tc.function.arguments,
      }));
    }
    return out;
  }
}

/** RetryableError is a 429, a 5xx, or a failed connection. */
export class RetryableError extends Error {
  readonly afterMs: number;

  constructor(message: string, opts: { afterMs?: number; cause?: unknown } = {}) {
    super(message, { cause: opts.cause });
    this.name = "RetryableError";
    this.afterMs = opts.afterMs ?? 0;
  }
}

/** openAIFromEnv reads LLM_API_KEY (required) and LLM_BASE_URL (optional). */
export function openAIFromEnv(): OpenAI {
  const apiKey = process.env.LLM_API_KEY;
  if (!apiKey) throw new Error("LLM_API_KEY is not set");
  return new OpenAI({ apiKey, baseUrl: process.env.LLM_BASE_URL || undefined });
}

// the chat completions wire format

interface WireToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

interface WireMessage {
  role: Message["role"];
  content: string | null;
  tool_calls?: WireToolCall[];
  tool_call_id?: string;
}

interface WireRequest {
  model: string;
  messages: WireMessage[];
  tools?: { type: "function"; function: { name: string; description: string; parameters: Record<string, unknown> } }[];
  temperature?: number;
  response_format?: { type: "json_schema"; json_schema: ResponseFormat };
}

interface WireResponse {
  choices?: {
    message: { content: string | null; tool_calls?: WireToolCall[] };
    finish_reason?: string;
  }[];
  usage?: Usage;
  error?: { message: string; type?: string };
}

function toWireMessage(m: Message): WireMessage {
  // an assistant message that only carries tool calls has null content
  const onlyToolCalls = !m.content && (m.tool_calls?.length ?? 0) > 0;
  const wm: WireMessage = { role: m.role, content: onlyToolCalls ? null : (m.content ?? "") };
  if (m.tool_calls?.length) {
    wm.tool_calls = m.tool_calls.map((tc) => ({
      id: tc.id,
      type: "function",
      function: { name: tc.name, arguments: tc.arguments },
    }));
  }
  if (m.tool_call_id) wm.tool_call_id = m.tool_call_id;
  return wm;
}

function truncate(s: string, n: number): string {
  return s.length <= n ? s : `${s.slice(0, n)}...`;
}

/** Stats is what a Recorder has seen so far. */
export interface Stats {
  calls: number;
  prompt_tokens: number;
  completion_tokens: number;
  duration_ms: number;
  errors: number;
}

interface TraceRecord {
  at: string;
  duration_ms: number;
  request: Request;
  response?: Response;
  error?: string;
}

/**
 * Recorder wraps a Client, counts calls and tokens, and writes every request
 * and response to a directory, one JSON file per call.
 */
export class Recorder implements Client {
  private readonly inner: Client;
  private readonly dir: string;
  private n = 0;
  private readonly totals: Stats = { calls: 0, prompt_tokens: 0, completion_tokens: 0, duration_ms: 0, errors: 0 };

  /** Wraps inner. If dir is empty nothing is written, only counted. */
  constructor(inner: Client, dir = "") {
    this.inner = inner;
    this.dir = dir;
  }

  /** stats returns the totals so far. */
  stats(): Stats {
    return { ...this.totals };
  }

  async chat(req: Request): Promise<Response> {
    const start = Date.now();
    let response: Response | undefined;
    let error: unknown;
    try {
      response = await this.inner.chat(req);
    } catch (err) {
      error = err;
    }
    const durationMs = Date.now() - start;

    const n = ++this.n;
    this.totals.calls++;
    this.totals.duration_ms += durationMs;
    this.totals.prompt_tokens += response?.usage?.prompt_tokens ?? 0;
    this.totals.completion_tokens += response?.usage?.completion_tokens ?? 0;
    if (error !== undefined) this.totals.errors++;

    if (this.dir) {
      const rec: TraceRecord = { at: new Date(start).toISOString(), duration_ms: durationMs, request: req };
      if (error !== undefined) rec.error = error instanceof Error ? error.message : String(error);
      else rec.response = response;
      try {
        await mkdir(this.dir, { recursive: true });
        await writeFile(path.join(this.dir, `${String(n).padStart(3, "0")}.json`), JSON.stringify(rec, null, 2));
      } catch {
        // tracing is best effort, like the Go harness
      }
    }

    if (error !== undefined) throw error;
    return response!;
  }
}
