import type { Client, Request, Response } from "./client.ts";

/**
 * Fake answers from a script, then from `fallback`. It is for tests: the
 * harness's own, and yours, for driving your loop through a scripted model
 * without spending tokens. It does not understand anything.
 */
export class Fake implements Client {
  script: Response[];
  fallback: Response;
  /** how many times chat was called */
  calls = 0;

  constructor(script: Response[] = [], fallback: Response = { content: "This is the fake model." }) {
    this.script = script;
    this.fallback = fallback;
  }

  async chat(_req: Request): Promise<Response> {
    const i = this.calls++;
    return i < this.script.length ? this.script[i]! : this.fallback;
  }
}

/**
 * fake returns a Fake that replays script in order, then keeps returning a
 * fixed line of text with no tool calls.
 */
export function fake(...script: Response[]): Fake {
  return new Fake(script);
}
