// The transcript module holds the shapes the harness feeds the agent and the
// shapes the agent returns. Field names follow what voice stacks already use:
// role and item type from the OpenAI Realtime API, is_final and speech_final
// from Deepgram, interrupted from LiveKit, prosody from Hume, and
// generation_config on the output from Cartesia.

import { readFile } from "node:fs/promises";

/**
 * Transcript is the call so far, as of `now`.
 *
 * Reading it:
 *
 * - A user item with is_final false is text still changing; is_final true
 *   and speech_final false is stable text from someone still talking;
 *   speech_final true means the speaker paused.
 * - A large gap between now and the last item's end, with speech_final
 *   true, is silence.
 * - A user item whose start falls inside an assistant item's range is a
 *   barge-in; that assistant item carries interrupted true.
 * - Two user items in a row is a caller who paused and went on.
 * - function_call and function_call_output items are API calls made
 *   earlier in the call, named by API method ("customers.identify",
 *   "bookings.create"), with the request as arguments and the response,
 *   or {"error", "detail"}, as output. Whatever tools you expose to the
 *   model, the transcript records what reached the API.
 *
 * Callers are people. They hedge, restart, correct themselves, talk to
 * someone in the room, and change the plan halfway.
 */
export interface Transcript {
  call_id: string;
  /** "2026-09-28", fixed per fixture so date words resolve */
  today: string;
  /** seconds since the call started */
  now: number;
  caller: Caller;
  items: Item[];
}

/** Caller is what the phone system knows before anyone speaks. */
export interface Caller {
  phone: string;
}

export type ItemType = "message" | "function_call" | "function_call_output";
export type Role = "user" | "assistant";

/**
 * Item is one conversation item. `type` decides which fields are set.
 *
 *     message               role, start, end, content, and either interrupted
 *                           (assistant) or is_final, speech_final, confidence,
 *                           prosody (user)
 *     function_call         call_id, name, arguments, start
 *     function_call_output  call_id, output, end
 *
 * Function calls are API calls made earlier in the call, named by API method
 * ("customers.identify", "bookings.create"), with the request as arguments
 * and the response, or {"error", "detail"}, as output.
 */
export interface Item {
  type: ItemType;

  role?: Role;
  start?: number;
  end?: number;
  content?: string;
  /** assistant: cut off by a barge-in */
  interrupted?: boolean;
  /** user: transcript text is stable */
  is_final?: boolean;
  /** user: the speaker paused */
  speech_final?: boolean;
  confidence?: number;
  prosody?: Prosody;

  call_id?: string;
  name?: string;
  /** JSON text */
  arguments?: string;
  /** JSON text */
  output?: string;
}

/** Prosody is a rough read of how the line was said, not what was said. */
export interface Prosody {
  emotions: Emotion[];
}

/** Emotion is one named dimension and its score in [0, 1]. */
export interface Emotion {
  name: string;
  score: number;
}

/**
 * Action is what the receptionist does next. It is a record with a "type"
 * key so new action types and new fields need no schema change. The harness
 * runs each action through the handler registered for its type; an action
 * with no handler fails the fixture. say, wait, transfer and endCall below
 * build the four the shipped handlers know; build others by hand.
 */
export type Action = { type: string } & Record<string, unknown>;

/**
 * Effect is what a handler produced when it carried out an action. The
 * fixtures assert on effects.
 */
export type Effect = { type: string } & Record<string, unknown>;

/** say speaks text. Emotion and speed are hints to the TTS; speed is 0.6 to 1.5. */
export function say(text: string, emotion: string, speed: number): Action {
  return { type: "say", text, generation_config: { emotion, speed } };
}

/** wait says nothing for a while and lets the caller finish or think. */
export function wait(seconds: number): Action {
  return { type: "wait", seconds };
}

/** transfer hands the call to a human queue. The queue name is yours to choose. */
export function transfer(to: string, reason: string): Action {
  return { type: "transfer", to, reason };
}

/** endCall hangs up. */
export function endCall(reason: string): Action {
  return { type: "end_call", reason };
}

/**
 * loadJson reads a transcript from a JSON file, for your own tests. The
 * fixtures carry their transcripts inline in YAML with the same keys.
 */
export async function loadJson(path: string): Promise<Transcript> {
  const raw = await readFile(path, "utf8");
  try {
    return JSON.parse(raw) as Transcript;
  } catch (err) {
    throw new Error(`${path}: ${(err as Error).message}`, { cause: err });
  }
}
