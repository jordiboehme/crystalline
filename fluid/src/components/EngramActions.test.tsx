import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { EngramDetail } from "../api/engram";
import { EngramActions, downloadName } from "./EngramActions";

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

describe("utility actions", () => {
  it("derives the download filename from the permalink slug", () => {
    expect(downloadName("notes/deep/gamma")).toBe("gamma.md");
    expect(downloadName("alpha")).toBe("alpha.md");
  });

  it("downloads the exact detail content as markdown bytes", async () => {
    const url = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:fake");
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    render(<EngramActions engram={ENGRAM} />);
    await userEvent.click(
      screen.getByRole("button", { name: "Download as Markdown" }),
    );
    const blob = url.mock.calls[0]?.[0] as Blob;
    expect(blob.type).toBe("text/markdown");
    expect(await blob.text()).toBe(ENGRAM.content);
  });

  it("copies the page address on Share and confirms in a live region", async () => {
    const write = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText: write } });
    render(<EngramActions engram={ENGRAM} />);
    await userEvent.click(screen.getByRole("button", { name: "Share link" }));
    expect(write).toHaveBeenCalledWith(
      expect.stringContaining("/d/eng/e/notes/deep/gamma"),
    );
    expect(await screen.findByText("Link copied")).toBeInTheDocument();
  });

  it("prints through the browser", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    render(<EngramActions engram={ENGRAM} />);
    await userEvent.click(screen.getByRole("button", { name: "Print view" }));
    expect(print).toHaveBeenCalled();
  });
});
