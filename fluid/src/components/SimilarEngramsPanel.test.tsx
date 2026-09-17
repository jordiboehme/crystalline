/**
 * The "Similar engrams" panel: what it draws, and what it never does.
 *
 * It teaches rather than decides - the guidance names the four things a
 * reader might do with a neighbour (merge, supersede, link, nothing), and
 * the panel itself does none of them. It renders nothing at all when there
 * is nothing to say, and it never blocks the editor: dismissing it is the
 * only thing it can do to itself.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";

import { engramRoute } from "../paths";
import { Tooltips } from "./primitives";
import { SimilarEngramsPanel } from "./SimilarEngramsPanel";

function renderPanel(ui: ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>, { wrapper: Tooltips });
}

describe("SimilarEngramsPanel", () => {
  it("links each neighbour and dismisses", async () => {
    const onDismiss = vi.fn();
    renderPanel(
      <SimilarEngramsPanel
        similar={[
          {
            domain: "eng",
            permalink: "retry-queue-gotcha",
            title: "Retry queue gotcha",
            status: "stable",
            type: "engram",
          },
        ]}
        guidance="read the one that fits"
        onDismiss={onDismiss}
      />,
    );
    expect(
      screen.getByRole("status", { name: "Similar engrams" }),
    ).toBeInTheDocument();
    expect(screen.getByText("read the one that fits")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: /Retry queue gotcha/ }),
    ).toHaveAttribute("href", engramRoute("eng", "retry-queue-gotcha"));
    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onDismiss).toHaveBeenCalledOnce();
  });

  it("shows the status and type beside each neighbour", () => {
    renderPanel(
      <SimilarEngramsPanel
        similar={[
          {
            domain: "eng",
            permalink: "retry-queue-gotcha",
            title: "Retry queue gotcha",
            status: "stable",
            type: "engram",
          },
        ]}
        guidance="read the one that fits"
        onDismiss={() => {}}
      />,
    );
    expect(screen.getByText("stable")).toBeInTheDocument();
    expect(screen.getByText("engram")).toBeInTheDocument();
  });

  it("renders nothing for an empty list", () => {
    const { container } = renderPanel(
      <SimilarEngramsPanel similar={[]} guidance={null} onDismiss={() => {}} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("is reachable and dismissible from the keyboard", async () => {
    const onDismiss = vi.fn();
    const user = userEvent.setup();
    renderPanel(
      <SimilarEngramsPanel
        similar={[
          {
            domain: "eng",
            permalink: "retry-queue-gotcha",
            title: "Retry queue gotcha",
            status: "stable",
            type: "engram",
          },
        ]}
        guidance="read the one that fits"
        onDismiss={onDismiss}
      />,
    );
    await user.tab();
    expect(screen.getByRole("button", { name: "Dismiss" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
