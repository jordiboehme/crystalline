/**
 * The floating bar's chunk is asked for only once there is something for it
 * to stand by: before the first context the dynamic import has not run.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";

import { FloatingBarSlot } from "./FloatingBarSlot";

const loaded = vi.hoisted(() => vi.fn());

vi.mock("./FloatingBar", () => {
  loaded();
  return {
    default: () => <div role="toolbar" aria-label="Quick format" />,
  };
});

describe("FloatingBarSlot", () => {
  test("does not load the bar before a context exists", async () => {
    const { rerender } = render(
      <FloatingBarSlot main={null} context={null} wikilinks />,
    );
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(loaded).not.toHaveBeenCalled();
    expect(screen.queryByRole("toolbar")).toBeNull();
    rerender(
      <FloatingBarSlot
        main={null}
        context={{ kind: "text", rect: new DOMRect(0, 0, 10, 10) }}
        wikilinks
      />,
    );
    expect(
      await screen.findByRole("toolbar", { name: "Quick format" }),
    ).toBeInTheDocument();
    expect(loaded).toHaveBeenCalledTimes(1);
  });
});
