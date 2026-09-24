import {
  fireEvent,
  getDefaultNormalizer,
  render,
  screen,
} from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { CrtReader } from "./CrtReader";

/** A body long enough that every section can sit at the top of the screen. */
const BODY = [
  "# Station log",
  "Intro paragraph.",
  ...Array.from({ length: 40 }, (_, i) => `- entry ${String(i)}`),
  "## Scope",
  "What the scope covers.",
  ...Array.from({ length: 40 }, (_, i) => `- scope ${String(i)}`),
  "## Scope",
  "The second scope.",
  ...Array.from({ length: 40 }, (_, i) => `- again ${String(i)}`),
].join("\n");

/** The text of every line the screen shows, top to bottom. */
function visibleLines(): string[] {
  return Array.from(
    screen.getByTestId("crt-lines").children,
    (row) => row.textContent,
  );
}

function renderReader(
  overrides: Partial<Parameters<typeof CrtReader>[0]> = {},
) {
  const onOpenFluid = vi.fn();
  const onClose = vi.fn();
  render(
    <CrtReader
      title="Station log"
      markdown={BODY}
      section={null}
      onOpenFluid={onOpenFluid}
      onClose={onClose}
      look="phosphor"
      {...overrides}
    />,
  );
  return { onOpenFluid, onClose };
}

describe("CrtReader", () => {
  it("renders the title, the lines and the footer", () => {
    renderReader();
    expect(screen.getByRole("dialog", { name: "Station log" })).toBeVisible();
    expect(visibleLines().slice(0, 3)).toEqual([
      "Station log",
      "Intro paragraph.",
      "- entry 0",
    ]);
    expect(
      screen.getByText("W/S SCROLL  F OPEN IN FLUID  ESC CLOSE", {
        normalizer: getDefaultNormalizer({ collapseWhitespace: false }),
      }),
    ).toBeInTheDocument();
  });

  it("opens at the terminal's section", () => {
    renderReader({ section: { heading: "Scope", occurrence: 1 } });
    expect(visibleLines().slice(0, 2)).toEqual(["Scope", "The second scope."]);
  });

  it("opens at a section in the last screen of a short document", () => {
    renderReader({
      markdown: "## A\none\n## B\ntwo",
      section: { heading: "B", occurrence: 0 },
    });
    expect(visibleLines()).toEqual(["B", "two"]);
    fireEvent.keyDown(window, { code: "KeyS" });
    expect(visibleLines()[0]).toBe("B");
    fireEvent.keyDown(window, { code: "KeyW" });
    expect(visibleLines()[0]).toBe("one");
  });

  it("opens at the top when the section is not in the text", () => {
    renderReader({ section: { heading: "Gone", occurrence: 0 } });
    expect(visibleLines()[0]).toBe("Station log");
  });

  it("uppercases headings for the PETSCII look", () => {
    renderReader({
      look: "petscii",
      section: { heading: "Scope", occurrence: 0 },
    });
    expect(visibleLines().slice(0, 2)).toEqual([
      "SCOPE",
      "What the scope covers.",
    ]);
  });

  it("scrolls by line and by screen, and stops at the top", () => {
    renderReader({ section: { heading: "Scope", occurrence: 0 } });
    fireEvent.keyDown(window, { code: "KeyS" });
    expect(visibleLines()[0]).toBe("What the scope covers.");
    fireEvent.keyDown(window, { code: "ArrowDown" });
    expect(visibleLines()[0]).toBe("- scope 0");
    fireEvent.keyDown(window, { code: "KeyW" });
    fireEvent.keyDown(window, { code: "ArrowUp" });
    expect(visibleLines()[0]).toBe("Scope");
    const rows = visibleLines().length;
    fireEvent.keyDown(window, { code: "PageDown" });
    // "Scope", its paragraph, then "- scope 0": a screen further down is
    // the entry `rows - 2`.
    expect(visibleLines()[0]).toBe(`- scope ${String(rows - 2)}`);
    expect(visibleLines().length).toBe(rows);
    fireEvent.keyDown(window, { code: "PageUp" });
    expect(visibleLines()[0]).toBe("Scope");
    for (let i = 0; i < 10; i++) {
      fireEvent.keyDown(window, { code: "PageUp" });
    }
    expect(visibleLines()[0]).toBe("Station log");
  });

  it("scrolls with the wheel", () => {
    renderReader();
    fireEvent.wheel(window, { deltaY: 3, deltaMode: 1 });
    expect(visibleLines()[0]).toBe("- entry 1");
  });

  it("prevents the page's own handling of its keys", () => {
    renderReader();
    const event = new KeyboardEvent("keydown", {
      code: "KeyS",
      cancelable: true,
    });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  it("calls onOpenFluid on F and onClose on Escape", () => {
    const { onOpenFluid, onClose } = renderReader();
    fireEvent.keyDown(window, { code: "KeyF" });
    expect(onOpenFluid).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(window, { code: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("leaves browser shortcuts with a modifier alone", () => {
    const { onOpenFluid, onClose } = renderReader();
    for (const modifier of ["metaKey", "ctrlKey", "altKey"] as const) {
      const event = new KeyboardEvent("keydown", {
        code: "KeyF",
        [modifier]: true,
        cancelable: true,
      });
      window.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
    }
    fireEvent.keyDown(window, { code: "KeyW", ctrlKey: true });
    fireEvent.keyDown(window, { code: "Escape", metaKey: true });
    fireEvent.keyDown(window, { code: "KeyS", altKey: true });
    expect(onOpenFluid).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(visibleLines()[0]).toBe("Station log");
  });

  it("stops listening once unmounted", () => {
    const onClose = vi.fn();
    const { unmount } = render(
      <CrtReader
        title="t"
        markdown="text"
        section={null}
        onOpenFluid={() => undefined}
        onClose={onClose}
        look="phosphor"
      />,
    );
    unmount();
    fireEvent.keyDown(window, { code: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("renders markup in the content as literal text", () => {
    renderReader({
      title: "<b>title</b>",
      markdown: "Look: <img src=x onerror=alert(1)> here",
    });
    expect(
      screen.getByText("Look: <img src=x onerror=alert(1)> here"),
    ).toBeInTheDocument();
    expect(screen.getByText("<b>title</b>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("b")).toBeNull();
  });
});
