/**
 * The rail's account of what this engram carries with it.
 *
 * What it lists is what the BODY references, resolved against what the domain
 * actually holds - not the domain's whole attachment list, which belongs to
 * every engram in it, and not the body's references alone, which would claim a
 * file exists because somebody wrote its name.
 *
 * The two states that follow from that are the ones under test: referenced and
 * present, which is a file to open and a size to read, and referenced and
 * missing, which is a fact about the knowledge base rather than an error on
 * this page. Deleting says out loud what it does not do - the references in
 * the prose stay written until a human edits them, and the maintenance sweep
 * is what will point at them.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import { AttachmentsSection } from "./AttachmentsSection";
import { Tooltips } from "./primitives";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

const BODY = [
  "![Shot](assets/2026/08/shot.png#right,w=50%)",
  "",
  "The [deck](assets/2026/08/deck.pdf) says more.",
].join("\n");

/** What the domain holds, in the shape the attachments route answers with. */
function listing(paths: string[]) {
  return {
    attachments: paths.map((path, index) => ({
      path,
      mime: path.endsWith(".png") ? "image/png" : "application/pdf",
      size: 1024 * (index + 2),
      modified: "2026-08-18T10:00:00Z",
      sha256: "abc",
    })),
  };
}

function draw(body = BODY, canDelete = true) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return {
    client,
    ...render(
      <QueryClientProvider client={client}>
        <AttachmentsSection domain="eng" body={body} canDelete={canDelete} />
      </QueryClientProvider>,
      { wrapper: Tooltips },
    ),
  };
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the rail's attachments", () => {
  it("lists what the body references, with size and kind", async () => {
    apiMock.mockResolvedValue(
      listing([
        "assets/2026/08/shot.png",
        "assets/2026/08/deck.pdf",
        "assets/spare.png",
      ]),
    );
    draw();
    // The size arrives with the listing; the names are the body's own, so
    // waiting for a size is waiting for the whole row to be answered.
    expect(await screen.findByText("2.0 KiB")).toBeInTheDocument();
    expect(screen.getByText("shot.png")).toBeInTheDocument();
    expect(screen.getByText("deck.pdf")).toBeInTheDocument();
    // The domain's other files belong to whatever engram references them.
    expect(screen.queryByText("spare.png")).toBeNull();
    expect(screen.getByText("png")).toBeInTheDocument();
    expect(screen.getByText("pdf")).toBeInTheDocument();
  });

  it("matches a reference by its path, fragment and all stripped", async () => {
    apiMock.mockResolvedValue(listing(["assets/2026/08/shot.png"]));
    draw("![Shot](assets/2026/08/shot.png#left,w=25%)");
    const link = await screen.findByRole("link", { name: /shot\.png/ });
    expect(link).toHaveAttribute(
      "href",
      "/api/v1/domains/eng/files/assets/2026/08/shot.png",
    );
    expect(link).toHaveAttribute("target", "_blank");
  });

  it("resolves a percent-encoded reference the way the page draws it", async () => {
    // The rail reads the raw source and the reading view is handed a
    // micromark-normalized URL; both go through the one decode, so a
    // hand-written escape lists as the file it draws rather than as missing.
    apiMock.mockResolvedValue(listing(["assets/2026/08/設計.png"]));
    draw("![Shot](assets/2026/08/%E8%A8%AD%E8%A8%88.png)");
    expect(await screen.findByText("2.0 KiB")).toBeInTheDocument();
    expect(screen.getByText("設計.png")).toBeInTheDocument();
    expect(screen.queryByText("missing")).toBeNull();
  });

  it("lists a reference written with a leading ./ as the file it names", async () => {
    apiMock.mockResolvedValue(listing(["assets/2026/08/shot.png"]));
    draw("![Shot](./assets/2026/08/shot.png#right)");
    expect(await screen.findByText("2.0 KiB")).toBeInTheDocument();
    expect(screen.queryByText("missing")).toBeNull();
    expect(screen.getByRole("link", { name: /shot\.png/ })).toHaveAttribute(
      "href",
      "/api/v1/domains/eng/files/assets/2026/08/shot.png",
    );
  });

  it("says a referenced file is missing rather than pretending it is there", async () => {
    apiMock.mockResolvedValue(listing(["assets/2026/08/shot.png"]));
    draw();
    // Missing is a claim, so it is only made once the listing has answered.
    expect(await screen.findByText("missing")).toBeInTheDocument();
    expect(screen.getByText("deck.pdf")).toBeInTheDocument();
    // Nothing to open and nothing to delete: there is no file.
    expect(screen.queryByRole("link", { name: /deck\.pdf/ })).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Remove deck.pdf" }),
    ).toBeNull();
  });

  it("draws no section at all when the body references nothing", () => {
    draw("Just prose, no attachments.");
    expect(screen.queryByText("Attachments")).toBeNull();
    // And asks the server nothing.
    expect(apiMock).not.toHaveBeenCalled();
  });

  it("asks before removing, and says what removing does not do", async () => {
    apiMock.mockResolvedValue(listing(["assets/2026/08/shot.png"]));
    draw("![Shot](assets/2026/08/shot.png)");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Remove shot.png" }),
    );
    expect(
      screen.getByText(
        "Remove shot.png? Engrams keep their references until edited; evolve will flag them.",
      ),
    ).toBeInTheDocument();
    // Asking is not doing: only the listing has been read.
    expect(apiMock).toHaveBeenCalledTimes(1);
  });

  it("deletes the file and reads the listing again", async () => {
    apiMock.mockResolvedValue(listing(["assets/2026/08/shot.png"]));
    draw("![Shot](assets/2026/08/shot.png)");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Remove shot.png" }),
    );
    await user.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith(
        "/domains/eng/files/assets/2026/08/shot.png",
        { method: "DELETE" },
      );
    });
    // The listing is stale the moment the file is gone, so it is read again.
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledTimes(3);
    });
  });

  it("keeps the file when the question is answered with no", async () => {
    apiMock.mockResolvedValue(listing(["assets/2026/08/shot.png"]));
    draw("![Shot](assets/2026/08/shot.png)");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Remove shot.png" }),
    );
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(
      screen.getByRole("button", { name: "Remove shot.png" }),
    ).toBeInTheDocument();
    expect(apiMock).toHaveBeenCalledTimes(1);
  });

  it("offers no delete control to a reader who may not write", async () => {
    apiMock.mockResolvedValue(listing(["assets/2026/08/shot.png"]));
    draw("![Shot](assets/2026/08/shot.png)", false);
    expect(await screen.findByText("shot.png")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Remove shot.png" }),
    ).toBeNull();
  });
});
