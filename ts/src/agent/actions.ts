import type { Client } from "../api/client.ts";
import type { Action, Effect, Transcript } from "../transcript/types.ts";

/** HandlerContext is what a handler can reach while carrying out an action. */
export interface HandlerContext {
  api: Client;
  transcript: Transcript;
}

/** Handler carries out one action and reports what happened as an Effect. */
export type Handler = (action: Action, hc: HandlerContext) => Effect | Promise<Effect>;

/**
 * handlers maps an action type to the code that carries it out. The harness
 * runs every action decide returns through the handler for its type and
 * asserts on the effects. An action with no handler fails the fixture.
 *
 * These four are the starting set. Add your own, change these, or wrap them.
 */
export const handlers: Record<string, Handler> = {
  say: (a) => ({ type: "speech", text: a.text, generation_config: a.generation_config }),
  wait: (a) => ({ type: "silence", seconds: a.seconds }),
  transfer: (a) => ({ type: "transfer", to: a.to, reason: a.reason }),
  end_call: (a) => ({ type: "hangup", reason: a.reason }),
};
