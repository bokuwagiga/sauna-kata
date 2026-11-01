import type { Client } from "../llm/client.ts";
import type { Effect, Transcript } from "../transcript/types.ts";

/** Verdict is the judge's answer on one criterion. */
export interface Verdict {
  criterion: string;
  pass: boolean;
  reason: string;
}

/** JudgeResult is every verdict plus, when something went wrong, why. */
export interface JudgeResult {
  verdicts: Verdict[];
  error?: string;
}

const JUDGE_SYSTEM = `You grade one reply from a phone receptionist at a mobile sauna rental company.
You see the call so far and the receptionist's latest reply. Each criterion is a statement about that reply.
Mark it pass if the statement is true of the reply and fail if it is not, with a one-sentence reason.
Judge only the latest reply, using the call for context. Be strict about the substance and lenient about wording.`;

/**
 * verdictSchema builds the judge's answer shape for these criteria: one
 * required property per criterion, named criterion_1, criterion_2, ..., each
 * carrying the criterion text as its description. Strict mode then guarantees
 * exactly one verdict per criterion. Inside a verdict, reason comes before
 * pass so the model writes its reasoning before it commits.
 */
export function verdictSchema(criteria: string[]): Record<string, unknown> {
  return {
    type: "object",
    properties: Object.fromEntries(
      criteria.map((cr, i) => [
        criterionKey(i),
        {
          type: "object",
          description: cr,
          properties: {
            reason: { type: "string", description: "one sentence" },
            pass: { type: "boolean" },
          },
          required: ["reason", "pass"],
          additionalProperties: false,
        },
      ]),
    ),
    required: criteria.map((_, i) => criterionKey(i)),
    additionalProperties: false,
  };
}

function criterionKey(i: number): string {
  return `criterion_${i + 1}`;
}

/** judge asks the model whether the last reply meets each criterion. */
export async function judge(c: Client, tr: Transcript, effects: Effect[], criteria: string[]): Promise<JudgeResult> {
  const lines: string[] = ["Call so far:"];
  for (const it of tr.items) {
    switch (it.type) {
      case "message":
        lines.push(`${it.role === "assistant" ? "receptionist" : "caller"}: ${it.content}`);
        break;
      case "function_call":
        lines.push(`[receptionist's system called ${it.name} with ${it.arguments}]`);
        break;
      case "function_call_output":
        lines.push(`[result: ${it.output}]`);
        break;
    }
  }
  const said: string[] = [];
  for (const e of effects) {
    switch (e.type) {
      case "speech":
        said.push(String(e.text));
        break;
      case "silence":
        said.push("(says nothing, waits)");
        break;
      case "transfer":
        said.push("(transfers the call to a person)");
        break;
      case "hangup":
        said.push("(hangs up)");
        break;
    }
  }
  lines.push("", "Receptionist's latest reply:", said.length ? said.join(" ") : "(nothing)");
  lines.push("", "Criteria:", ...criteria.map((cr, i) => `${i + 1}. ${cr}`));

  let content: string;
  try {
    const resp = await c.chat({
      messages: [
        { role: "system", content: JUDGE_SYSTEM },
        { role: "user", content: lines.join("\n") },
      ],
      temperature: 0,
      response_format: { name: "verdict", schema: verdictSchema(criteria), strict: true },
    });
    content = resp.content ?? "";
  } catch (err) {
    return { verdicts: [], error: err instanceof Error ? err.message : String(err) };
  }

  let parsed: Record<string, { reason: string; pass: boolean } | undefined>;
  try {
    parsed = JSON.parse(content);
  } catch {
    return { verdicts: [], error: `unreadable verdict: ${truncate(content, 200)}` };
  }
  let missing = 0;
  const verdicts = criteria.map((criterion, i): Verdict => {
    const v = parsed[criterionKey(i)];
    if (!v) {
      missing++;
      return { criterion, pass: false, reason: "no verdict given" };
    }
    return { criterion, pass: v.pass, reason: v.reason };
  });
  if (missing > 0) {
    return { verdicts, error: `verdict missing for ${missing} of ${criteria.length} criteria` };
  }
  return { verdicts };
}

function truncate(s: string, n: number): string {
  return s.length <= n ? s : `${s.slice(0, n)}...`;
}
