/**
 * The three utilities, as everything that runs them sees them: through the
 * handlers ref. The component draws no controls of its own - the page's
 * overflow menu and the command palette are what call these - so what is
 * pinned here is the ref contract and the live region beside it.
 */

import { act, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { EngramDetail } from "../api/engram";
import { EngramActions, downloadName } from "./EngramActions";
import type { EngramActionHandlers } from "./EngramActions";

const ENGRAM = {
  domain: "eng",
  permalink: "notes/deep/gamma",
  title: "Gamma",
  url: "crystalline://eng/notes/deep/gamma",
  path: "notes/deep/gamma.md",
  content: "---\ntitle: Gamma\n---\n\nBody.\n",
  checksum: "abc",
  frontmatter: {
    type: null,
    status: null,
    tags: [],
    salience: null,
    validFrom: null,
    validTo: null,
    staleAfter: null,
    verified: [],
    generatedBy: null,
  },
  observations: [],
  relations: [],
  links: [],
  inboundCount: 0,
  inboundRefs: [],
} satisfies EngramDetail;

afterEach(() => {
  vi.restoreAllMocks();
});

/** Mount it and keep the handlers it hands out. */
function mounted() {
  const handlers = createRef<EngramActionHandlers | null>();
  render(<EngramActions engram={ENGRAM} handlers={handlers} />);
  return handlers;
}

describe("utility actions", () => {
  it("derives the download filename from the permalink slug", () => {
    expect(downloadName("notes/deep/gamma")).toBe("gamma.md");
    expect(downloadName("alpha")).toBe("alpha.md");
  });

  it("draws no controls of its own, only the region that announces them", () => {
    mounted();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(
      screen.getByRole("status", { name: "Share link result" }),
    ).toBeInTheDocument();
  });

  it("downloads the exact detail content as markdown bytes", async () => {
    const url = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:fake");
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const handlers = mounted();
    act(() => {
      handlers.current?.download();
    });
    const blob = url.mock.calls[0]?.[0] as Blob;
    expect(blob.type).toBe("text/markdown");
    expect(await blob.text()).toBe(ENGRAM.content);
  });

  it("copies the page address on Share and confirms in a live region", async () => {
    const write = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText: write } });
    const handlers = mounted();
    act(() => {
      handlers.current?.share();
    });
    expect(write).toHaveBeenCalledWith(
      expect.stringContaining("/d/eng/e/notes/deep/gamma"),
    );
    expect(await screen.findByText("Link copied")).toBeInTheDocument();
  });

  it("degrades gracefully when the Clipboard API is unavailable", async () => {
    const original = navigator.clipboard;
    // An insecure or older context carries no `navigator.clipboard` at all,
    // which throws synchronously the moment `.writeText` is read off it -
    // before any `.then`/`.catch` could run. Deleting it here is what an
    // actual such context looks like, rather than a rejected promise.
    delete (navigator as { clipboard?: unknown }).clipboard;
    const handlers = mounted();
    act(() => {
      handlers.current?.share();
    });
    expect(await screen.findByText("Copy refused")).toBeInTheDocument();
    Object.assign(navigator, { clipboard: original });
  });

  it("prints through the browser", () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const handlers = mounted();
    act(() => {
      handlers.current?.print();
    });
    expect(print).toHaveBeenCalled();
  });
});
