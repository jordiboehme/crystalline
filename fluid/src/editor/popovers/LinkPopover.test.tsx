/**
 * The "Link" and "Wiki link" popovers: their fields, buttons and what each
 * writes (only the span it edits, as one undo step).
 */

import { cleanup, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  keys,
  setCaret,
  undoOnce,
} from "../../test/markKit";
import { popoverView } from "../../test/popoverKit";

vi.mock("../../api/search", async (original) => ({
  ...(await original<typeof import("../../api/search")>()),
  fetchSearch: vi.fn(() =>
    Promise.resolve({
      hits: [
        { title: "Newer", domain: "eng" },
        { title: "Newest", domain: "other" },
      ],
    }),
  ),
}));

afterEach(() => {
  cleanup();
  cleanupMarkViews();
});

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];
/** Both modes, solo and in a room. */
const CASES = MODES.flatMap((mode) =>
  [false, true].map((room) => ({ mode, room })),
);

function linkText(): Element {
  const el = document.querySelector(".cm-link-text");
  if (!el) throw new Error("no link text is drawn");
  return el;
}

function press(el: Element): void {
  el.dispatchEvent(
    new MouseEvent("mousedown", { bubbles: true, cancelable: true }),
  );
}

describe.each(CASES)(
  "an inline link ($mode mode, room $room)",
  ({ mode, room }) => {
    it("shows its address and writes only the new one", async () => {
      const user = userEvent.setup();
      const view = popoverView('x\n[t](https://a.b "T") after', { mode, room });
      press(linkText());
      await screen.findByRole("dialog", { name: "Link" });
      const field = screen.getByRole("textbox", { name: "Address" });
      expect(field).toHaveValue("https://a.b");
      expect(screen.queryByRole("textbox", { name: "Link text" })).toBeNull();
      await user.clear(field);
      await user.type(field, "https://c.d{Enter}");
      expect(bytes(view.state)).toBe('x\n[t](https://c.d "T") after');
      undoOnce(view);
      expect(bytes(view.state)).toBe('x\n[t](https://a.b "T") after');
    });

    it("removes the link and keeps its words", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\na [t](u) b", { mode, room });
      press(linkText());
      await screen.findByRole("dialog", { name: "Link" });
      await user.click(screen.getByRole("button", { name: "Remove link" }));
      expect(bytes(view.state)).toBe("x\na t b");
      expect(screen.queryByRole("dialog")).toBeNull();
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\na [t](u) b");
    });

    it("opens a web address in a new tab, never in the editor", async () => {
      popoverView("x\n[t](https://a.b)", { mode, room });
      press(linkText());
      await screen.findByRole("dialog", { name: "Link" });
      const open = screen.getByRole("link", { name: "Open link" });
      expect(open).toHaveAttribute("href", "https://a.b");
      expect(open).toHaveAttribute("target", "_blank");
      expect(open).toHaveAttribute("rel", "noopener noreferrer");
    });

    it("offers no Open link for a script address", async () => {
      popoverView("x\n[t](javascript:alert(1))", { mode, room });
      press(linkText());
      await screen.findByRole("dialog", { name: "Link" });
      expect(screen.queryByRole("link", { name: "Open link" })).toBeNull();
      expect(screen.getByRole("button", { name: "Open link" })).toBeDisabled();
    });

    it("does not save an empty address", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n[t](u)", { mode, room });
      press(linkText());
      await screen.findByRole("dialog", { name: "Link" });
      await user.clear(screen.getByRole("textbox", { name: "Address" }));
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
      await user.keyboard("{Enter}");
      expect(bytes(view.state)).toBe("x\n[t](u)");
    });
  },
);

describe.each(CASES)(
  "a reference link ($mode mode, room $room)",
  ({ mode, room }) => {
    it("shows the shared address read only and can remove the link", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n[t][r]\n\n[r]: https://a.b", { mode, room });
      press(linkText());
      await screen.findByRole("dialog", { name: "Link" });
      const field = screen.getByRole("textbox", { name: "Address" });
      expect(field).toHaveValue("https://a.b");
      expect(field).toHaveAttribute("readonly");
      expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
      expect(screen.getByRole("link", { name: "Open link" })).toHaveAttribute(
        "href",
        "https://a.b",
      );
      await user.click(screen.getByRole("button", { name: "Remove link" }));
      expect(bytes(view.state)).toBe("x\nt\n\n[r]: https://a.b");
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\n[t][r]\n\n[r]: https://a.b");
    });
  },
);

describe.each(CASES)(
  "a new link at a caret ($mode mode, room $room)",
  ({ mode, room }) => {
    it("asks for the text first and writes both", async () => {
      const user = userEvent.setup();
      const view = popoverView("a  b", { mode, room });
      setCaret(view, 2);
      keys(view, "Mod-k");
      await screen.findByRole("dialog", { name: "Link" });
      const text = screen.getByRole("textbox", { name: "Link text" });
      expect(text).toHaveFocus();
      await user.type(text, "here");
      await user.type(
        screen.getByRole("textbox", { name: "Address" }),
        "https://x{Enter}",
      );
      expect(bytes(view.state)).toBe("a [here](https://x) b");
      undoOnce(view);
      expect(bytes(view.state)).toBe("a  b");
    });

    it("uses the address as the text when none is typed", async () => {
      const user = userEvent.setup();
      const view = popoverView("", { mode, room });
      keys(view, "Mod-k");
      await screen.findByRole("dialog", { name: "Link" });
      await user.type(
        screen.getByRole("textbox", { name: "Address" }),
        "https://x{Enter}",
      );
      expect(bytes(view.state)).toBe("[https://x](https://x)");
      undoOnce(view);
      expect(bytes(view.state)).toBe("");
    });
  },
);

describe.each([false, true])(
  "a crystalline address in a Reflection (room: %s)",
  (room) => {
    it("is edited like any other link", async () => {
      const user = userEvent.setup();
      const text =
        "## Night\n<!-- r:7f3a9c2e -->\n\nSee [n](crystalline://eng/a).\n";
      const view = popoverView(text, { adapter: "reflection", room });
      press(linkText());
      await screen.findByRole("dialog", { name: "Link" });
      const field = screen.getByRole("textbox", { name: "Address" });
      expect(field).toHaveValue("crystalline://eng/a");
      await user.clear(field);
      await user.type(field, "crystalline://eng/b{Enter}");
      expect(bytes(view.state)).toBe(text.replace("eng/a", "eng/b"));
      undoOnce(view);
      expect(bytes(view.state)).toBe(text);
    });
  },
);

describe.each(CASES)(
  "the Wiki link popover ($mode mode, room $room)",
  ({ mode, room }) => {
    function chip(): Element {
      const el = document.querySelector(".cm-wikilink");
      if (!el) throw new Error("no chip is drawn");
      return el;
    }

    it("writes a new target and keeps the alias", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\na [[Old|words]] b", { mode, room });
      press(chip());
      await screen.findByRole("dialog", { name: "Wiki link" });
      const field = screen.getByRole("combobox", { name: "Target" });
      expect(field).toHaveFocus();
      expect(field).toHaveValue("Old");
      await user.clear(field);
      // Enter with no suggestion picked saves what is typed.
      await user.type(field, "New{Enter}");
      expect(bytes(view.state)).toBe("x\na [[New|words]] b");
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\na [[Old|words]] b");
    });

    it("offers the title matches and writes the one picked", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n[[Old]]", { mode, room });
      press(chip());
      await screen.findByRole("dialog", { name: "Wiki link" });
      const field = screen.getByRole("combobox", { name: "Target" });
      await user.clear(field);
      await user.type(field, "New");
      await screen.findByRole("option", { name: /Newest/ });
      await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
      expect(field).toHaveValue("other:Newest");
      await user.keyboard("{Enter}");
      expect(bytes(view.state)).toBe("x\n[[other:Newest]]");
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\n[[Old]]");
    });

    it("removes the link and keeps the alias words", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n[[Old|words]]", { mode, room });
      press(chip());
      await screen.findByRole("dialog", { name: "Wiki link" });
      await user.click(screen.getByRole("button", { name: "Remove link" }));
      expect(bytes(view.state)).toBe("x\nwords");
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\n[[Old|words]]");
    });

    it("offers Open for the engram the chip names", async () => {
      popoverView("x\n[[Old]]", { mode, room });
      press(chip());
      await screen.findByRole("dialog", { name: "Wiki link" });
      // No resolver answers in this test: nothing to open yet.
      expect(screen.getByRole("button", { name: "Open" })).toBeDisabled();
    });
  },
);
