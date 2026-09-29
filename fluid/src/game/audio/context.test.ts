import { describe, expect, expectTypeOf, it } from "vitest";

import type { AudioContextLike, BaseAudioContextLike } from "./context";
import { FakeAudioContext } from "./testContext";

/*
 * Type-level checks: the real classes are never constructed (jsdom has no
 * WebAudio), so these assertions are enforced by `pnpm typecheck`, not by
 * the test run. The one runtime assertion keeps the file a real test.
 */
describe("the context interfaces", () => {
  // Mutation caught (by the typecheck): `close()` or a one-argument
  // `suspend()` moved into the base interface. The offline context has no
  // `close()` and its `suspend(time)` needs an argument, so it would stop
  // fitting the base and this file would fail to typecheck.
  it("fit both real contexts, the live one only the live interface", () => {
    expectTypeOf<AudioContext>().toExtend<AudioContextLike>();
    expectTypeOf<AudioContext>().toExtend<BaseAudioContextLike>();
    expectTypeOf<OfflineAudioContext>().toExtend<BaseAudioContextLike>();
    expectTypeOf<OfflineAudioContext>().not.toExtend<AudioContextLike>();
  });

  // Mutation caught: the fake drifting from the interface (a missing
  // factory or lifecycle method), which the typecheck rejects.
  it("fit the fake", () => {
    expectTypeOf<FakeAudioContext>().toExtend<AudioContextLike>();
    const ctx: AudioContextLike = new FakeAudioContext();
    expect(ctx.state).toBe("suspended");
  });
});
