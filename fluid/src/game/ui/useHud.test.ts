/**
 * The HUD hook after its screen went away: the game loop can still report a
 * connector change, and no timer may be left behind to set state later.
 */

import { act, renderHook } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useHud } from "./useHud";

describe("useHud", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts no hide timer once its screen is gone", () => {
    const { result, unmount } = renderHook(() => useHud());
    const sink = result.current.sink;
    act(() => {
      sink.connector(true, "Door");
    });
    unmount();
    sink.connector(false, "Door");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("still hides the connector after its minimum time while shown", () => {
    const { result } = renderHook(() => useHud());
    act(() => {
      result.current.sink.connector(true, "Door");
    });
    act(() => {
      result.current.sink.connector(false, "Door");
    });
    expect(result.current.connector.active).toBe(true);
    act(() => {
      vi.runAllTimers();
    });
    expect(result.current.connector.active).toBe(false);
  });

  it("keeps hiding the connector under StrictMode's second effect run", () => {
    const { result } = renderHook(() => useHud(), { wrapper: StrictMode });
    act(() => {
      result.current.sink.connector(true, "Door");
    });
    expect(result.current.connector.active).toBe(true);
    act(() => {
      result.current.sink.connector(false, "Door");
    });
    act(() => {
      vi.runAllTimers();
    });
    expect(result.current.connector.active).toBe(false);
  });
});
