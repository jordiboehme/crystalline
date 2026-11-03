import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useMarkSetting } from "./context";
import { MARK_SETTING_KEY, MarkSettingProvider } from "./MarkSettingProvider";

function Probe() {
  const { showOnEditLine, setShowOnEditLine } = useMarkSetting();
  return (
    <button
      type="button"
      onClick={() => {
        setShowOnEditLine(!showOnEditLine);
      }}
    >
      {showOnEditLine ? "on" : "off"}
    </button>
  );
}

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the marks setting", () => {
  it("is off by default", () => {
    render(
      <MarkSettingProvider>
        <Probe />
      </MarkSettingProvider>,
    );
    expect(screen.getByRole("button")).toHaveTextContent("off");
  });

  it("is remembered as on, removed when off, and read back on remount", async () => {
    const first = render(
      <MarkSettingProvider>
        <Probe />
      </MarkSettingProvider>,
    );
    await userEvent.click(screen.getByRole("button"));
    expect(localStorage.getItem(MARK_SETTING_KEY)).toBe("on");
    expect(MARK_SETTING_KEY).toBe("fluid-marks-on-edit-line");
    first.unmount();

    render(
      <MarkSettingProvider>
        <Probe />
      </MarkSettingProvider>,
    );
    expect(screen.getByRole("button")).toHaveTextContent("on");
    await userEvent.click(screen.getByRole("button"));
    expect(localStorage.getItem(MARK_SETTING_KEY)).toBeNull();
  });

  it("is off and does not throw when storage is denied", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    render(
      <MarkSettingProvider>
        <Probe />
      </MarkSettingProvider>,
    );
    expect(screen.getByRole("button")).toHaveTextContent("off");
    await userEvent.click(screen.getByRole("button"));
    expect(screen.getByRole("button")).toHaveTextContent("on");
  });
});
