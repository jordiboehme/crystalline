import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useLapsingState } from "./useLapsingState";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("useLapsingState", () => {
  it("holds a value until it is cleared", () => {
    const { result } = renderHook(() => useLapsingState<boolean>(1000));
    expect(result.current[0]).toBeNull();
    act(() => {
      result.current[1](true);
    });
    expect(result.current[0]).toBe(true);
    act(() => {
      result.current[1](null);
    });
    expect(result.current[0]).toBeNull();
  });

  it("lets a value lapse when nothing took it in time", () => {
    const { result } = renderHook(() => useLapsingState<boolean>(1000));
    act(() => {
      result.current[1](false);
    });
    act(() => {
      vi.advanceTimersByTime(999);
    });
    expect(result.current[0]).toBe(false);
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(result.current[0]).toBeNull();
  });

  it("starts the time again for a new value", () => {
    const { result } = renderHook(() => useLapsingState<boolean>(1000));
    act(() => {
      result.current[1](true);
    });
    act(() => {
      vi.advanceTimersByTime(800);
    });
    act(() => {
      result.current[1](false);
    });
    act(() => {
      vi.advanceTimersByTime(800);
    });
    expect(result.current[0]).toBe(false);
  });

  it("leaves no timer behind when it goes", () => {
    const { result, unmount } = renderHook(() =>
      useLapsingState<boolean>(1000),
    );
    act(() => {
      result.current[1](true);
    });
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
