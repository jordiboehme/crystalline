/**
 * The popover host's chunk is asked for only once a first request exists:
 * before it the dynamic import has not run.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";

import { ElementPopoverSlot } from "./ElementPopoverSlot";

const loaded = vi.hoisted(() => vi.fn());

vi.mock("./ElementPopover", () => {
  loaded();
  return {
    default: () => <div role="dialog" aria-label="Link" />,
  };
});

describe("ElementPopoverSlot", () => {
  test("does not load the host before the first request", async () => {
    const view = {} as never;
    const { rerender } = render(
      <ElementPopoverSlot main={null} request={null} />,
    );
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(loaded).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
    rerender(
      <ElementPopoverSlot
        main={view}
        request={{
          edit: {
            kind: "new-link",
            rect: new DOMRect(),
            range: { from: 0, to: 0 },
          },
          view,
        }}
      />,
    );
    expect(
      await screen.findByRole("dialog", { name: "Link" }),
    ).toBeInTheDocument();
    expect(loaded).toHaveBeenCalledTimes(1);
  });
});
