// The agent module is the receptionist. This is where the work happens.
//
// decide gets the call so far and returns what to do next. handlers, in
// actions.ts, carry those actions out. Everything else in the repo is
// plumbing you can use or ignore.

import type { Client as ApiClient } from "../api/client.ts";
import type { AvailabilityQuery, BookingRequest } from "../api/types.ts";
import type { Client as LlmClient, Message } from "../llm/client.ts";
import { runAgent, type Tool } from "../llm/loop.ts";
import { say, type Action, type Transcript } from "../transcript/types.ts";

/**
 * decide returns the receptionist's next actions given the transcript so far.
 *
 * What is here now is a starting point. It passes the happy-path fixture and
 * not much else.
 */
export async function decide(t: Transcript, api: ApiClient, llm: LlmClient): Promise<Action[]> {
  const units = await api.listUnits();
  const system =
    "You are the receptionist for Pirtis on Wheels, a mobile sauna rental. " +
    `Help the caller book a sauna. Today is ${t.today}. ` +
    `Units: ${JSON.stringify(units)}`;

  // every item becomes history, whether the caller finished the sentence or not
  const history: Message[] = [];
  for (const it of t.items) {
    switch (it.type) {
      case "message":
        history.push({ role: it.role ?? "user", content: it.content });
        break;
      case "function_call":
        history.push({ role: "assistant", content: `[called ${it.name} with ${it.arguments}]` });
        break;
      case "function_call_output":
        history.push({ role: "assistant", content: `[result: ${it.output}]` });
        break;
    }
  }

  const tools: Tool[] = [
    {
      name: "availability",
      description: "Free dates per unit between two dates, YYYY-MM-DD.",
      parameters: {
        type: "object",
        properties: { from: { type: "string" }, to: { type: "string" } },
        required: ["from", "to"],
      },
      // no people, no weight, no power. every unit comes back and the model picks.
      async execute(args) {
        const q = args as AvailabilityQuery;
        return { data: await api.availability({ from: q.from, to: q.to }) };
      },
    },
    {
      name: "book",
      description: "Create a booking.",
      parameters: {
        type: "object",
        properties: {
          customer_id: { type: "string" },
          unit_id: { type: "string" },
          start: { type: "string" },
          end: { type: "string" },
          people: { type: "integer" },
        },
        required: ["customer_id", "unit_id", "start", "end", "people"],
      },
      async execute(args) {
        const req = args as BookingRequest;
        req.site = { power_available: true }; // nobody asked
        req.tow_capacity_kg = 3500; // nobody asked
        return { data: await api.createBooking(req) };
      },
    },
  ];

  const res = await runAgent(llm, system, history, tools, 8);
  return res.terminal ?? [say(res.text, "neutral", 1.0)];
}
