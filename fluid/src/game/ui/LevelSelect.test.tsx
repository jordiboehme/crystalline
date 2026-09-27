/**
 * The level select, rendered over a real query client with only `api`
 * stubbed: the listing, the filter, the selection, the keys, and the
 * listing that is still loading or failed.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../../api/client";
import { DOMAINS_QUERY_KEY, readListing } from "../../api/domains";
import { answersFor, type Answer } from "../../test/harness";
import { LevelSelect } from "./LevelSelect";
import {
  LEVEL_ROWS,
  LEVELS_FAILED,
  LEVELS_FOOTER,
  LEVELS_LOADING,
  NO_SUCH_LEVEL,
} from "./levels";

vi.mock("../../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** A `GET /domains` payload naming these domains. */
function listing(names: string[]) {
  return {
    behavior: [],
    domains: names.map((name) => ({ name, kind: "file" })),
  };
}

/** Mounts the select over `answer` for `/domains`. */
function renderSelect(answer: Answer, current = "eng") {
  apiMock.mockImplementation(answersFor({ "/domains": answer }));
  const onJump = vi.fn<(domain: string) => void>();
  const onClose = vi.fn<() => void>();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <LevelSelect current={current} onJump={onJump} onClose={onClose} />
    </QueryClientProvider>,
  );
  const field = screen.getByRole("textbox", { name: "Domain name" });
  return { onJump, onClose, field, client };
}

/** The names of the rows shown, top to bottom. */
function rows(): string[] {
  return screen
    .queryAllByRole("option")
    .map((row) => row.firstElementChild?.textContent ?? "");
}

/** The name of the selected row. */
function selected(): string | undefined {
  return screen.getByRole("option", { selected: true }).firstElementChild
    ?.textContent;
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("LevelSelect", () => {
  it("lists the domains sorted by name, the first selected, the current one marked", async () => {
    const { field } = renderSelect(() => listing(["eng", "beta", "Alpha"]));
    expect(
      screen.getByRole("dialog", { name: "Jump to a domain" }),
    ).toBeVisible();
    expect(field).toHaveFocus();
    await screen.findAllByRole("option");
    expect(rows()).toEqual(["Alpha", "beta", "eng"]);
    expect(selected()).toBe("Alpha");
    const options = screen.getAllByRole("option");
    expect(within(options[2]!).getByText("HERE")).toBeInTheDocument();
    expect(within(options[0]!).queryByText("HERE")).toBeNull();
    expect(screen.getByText("1/3")).toBeInTheDocument();
  });

  // Final review Minor 1: the footer's double spaces collapse without
  // `whitespace-pre`. `jsdom` has no layout engine, so a plain text-content
  // check would pass either way; the class is what actually pins it.
  it("keeps the footer's double spaces from collapsing", async () => {
    renderSelect(() => listing(["a"]));
    await screen.findAllByRole("option");
    const footer = screen.getByText(
      (_, element) => element?.textContent === LEVELS_FOOTER,
    );
    expect(footer).toHaveClass("whitespace-pre");
  });

  it("filters by name ignoring case, and says NO SUCH LEVEL when nothing matches", async () => {
    const { field, onJump } = renderSelect(() =>
      listing(["eng", "beta", "Alpha"]),
    );
    await screen.findAllByRole("option");
    fireEvent.change(field, { target: { value: " E " } });
    expect(rows()).toEqual(["beta", "eng"]);
    expect(selected()).toBe("beta");

    fireEvent.change(field, { target: { value: "zzz" } });
    expect(screen.getByText(NO_SUCH_LEVEL)).toBeInTheDocument();
    expect(rows()).toEqual([]);
    expect(screen.getByText("0/0")).toBeInTheDocument();
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onJump).not.toHaveBeenCalled();
  });

  it("moves the selection with the arrows, clamped, resets it on a new filter, and jumps on Enter", async () => {
    const { field, onJump } = renderSelect(() => listing(["a", "b", "c"]));
    await screen.findAllByRole("option");
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(selected()).toBe("a");
    for (let i = 0; i < 5; i++) fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(selected()).toBe("c");
    // Two changes: React fires no change for a value that stays "".
    fireEvent.change(field, { target: { value: "x" } });
    fireEvent.change(field, { target: { value: "" } });
    expect(selected()).toBe("a");
    fireEvent.keyDown(field, { key: "ArrowDown" });
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onJump).toHaveBeenCalledTimes(1);
    expect(onJump).toHaveBeenCalledWith("b");
  });

  // Final review Minor 3: the arrow handler must step from the shown
  // clamp (`at`), not the stored `selected`, or a background refetch that
  // shrinks the list leaves the first arrow press looking like a no-op.
  it("steps the selection from the shown clamp, not the stored value, after a background refetch shrinks the list", async () => {
    const { field, client } = renderSelect(() =>
      listing(["a", "b", "c", "d", "e"]),
    );
    await screen.findAllByRole("option");
    for (let i = 0; i < 4; i++) fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(selected()).toBe("e");

    // A background refetch, not the player's own filter: the query's data
    // changes under the selection without going through `onChange`, which
    // is the only place that resets `selected`. The stored value stays 4,
    // clamped to "c" for display.
    act(() => {
      client.setQueryData(
        DOMAINS_QUERY_KEY,
        readListing(listing(["a", "b", "c"])),
      );
    });
    await waitFor(() => {
      expect(selected()).toBe("c");
    });

    // One ArrowUp must step from the shown "c" to "b". Stepping from the
    // stored 4 would clamp straight back to "c" and look like nothing
    // happened.
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(selected()).toBe("b");
  });

  it("jumps on a click, and a mousedown keeps the focus in the field", async () => {
    const { onJump } = renderSelect(() => listing(["a", "b"]));
    await screen.findAllByRole("option");
    const row = screen.getAllByRole("option")[1]!;
    // The real check: a mousedown outside the field is prevented, which is
    // what keeps the focus in a browser. `jsdom`'s `fireEvent` never moves
    // focus on its own, so asserting `toHaveFocus()` here would hold
    // whether or not the prevention fired.
    expect(fireEvent.mouseDown(row)).toBe(false);
    fireEvent.click(row);
    expect(onJump).toHaveBeenCalledWith("b");
  });

  it("closes on Esc, and not on its auto-repeat", async () => {
    const { field, onClose } = renderSelect(() => listing(["a"]));
    await screen.findAllByRole("option");
    fireEvent.keyDown(field, { key: "Escape", repeat: true });
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(field, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("leaves Ctrl, Cmd and Alt combinations to the browser", async () => {
    const { field, onClose, onJump } = renderSelect(() => listing(["a"]));
    await screen.findAllByRole("option");
    expect(fireEvent.keyDown(field, { key: "Enter", metaKey: true })).toBe(
      true,
    );
    fireEvent.keyDown(field, { key: "Escape", ctrlKey: true });
    expect(onJump).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  // Review Focus 2: the V that completed the word may still be held (C17).
  it("swallows the auto-repeat of the key that opened it", () => {
    const { field } = renderSelect(() => listing(["a"]));
    expect(
      fireEvent.keyDown(field, { key: "v", code: "KeyV", repeat: true }),
    ).toBe(false);
    expect(fireEvent.keyDown(field, { key: "e", code: "KeyE" })).toBe(true);
    expect(
      fireEvent.keyDown(field, { key: "e", code: "KeyE", repeat: true }),
    ).toBe(true);
  });

  // Review Focus 4.
  it("says LOADING DOMAINS while the listing loads", () => {
    const { field, onJump, onClose } = renderSelect(
      () => new Promise(() => {}),
    );
    expect(screen.getByText(LEVELS_LOADING)).toBeInTheDocument();
    expect(screen.queryByText(NO_SUCH_LEVEL)).toBeNull();
    fireEvent.change(field, { target: { value: "en" } });
    expect(field).toHaveValue("en");
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onJump).not.toHaveBeenCalled();
    fireEvent.keyDown(field, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("says ?DOMAIN LIST ERROR when the listing fails", async () => {
    const { field, onJump, onClose } = renderSelect(() => {
      throw new ApiProblem(500, "boom", "the index is down");
    });
    expect(await screen.findByText(LEVELS_FAILED)).toBeInTheDocument();
    expect(screen.queryByText(NO_SUCH_LEVEL)).toBeNull();
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onJump).not.toHaveBeenCalled();
    fireEvent.keyDown(field, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("finds a renamed domain by any of its names, labels it with both and jumps by the local one", async () => {
    const { field, onJump } = renderSelect(
      () => ({
        behavior: [],
        domains: [
          { name: "eng", kind: "file" },
          {
            name: "infra",
            kind: "file",
            canonical_name: "platform",
            aliases: ["old-infra"],
          },
        ],
      }),
      "old-infra",
    );
    await screen.findAllByRole("option");
    expect(rows()).toEqual(["eng", "platform (infra)"]);
    const options = screen.getAllByRole("option");
    expect(within(options[1]!).getByText("HERE")).toBeInTheDocument();
    expect(within(options[0]!).queryByText("HERE")).toBeNull();
    for (const query of ["platform", "old-infra", "INFRA"]) {
      fireEvent.change(field, { target: { value: query } });
      expect(rows()).toEqual(["platform (infra)"]);
    }
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onJump).toHaveBeenCalledWith("infra");
  });

  // Review Focus 5.
  it("one domain: one row, selected, marked and jumpable", async () => {
    const { field, onJump } = renderSelect(() => listing(["eng"]), "eng");
    await screen.findAllByRole("option");
    expect(rows()).toEqual(["eng"]);
    fireEvent.keyDown(field, { key: "ArrowDown" });
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(selected()).toBe("eng");
    expect(screen.getByText("HERE")).toBeInTheDocument();
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onJump).toHaveBeenCalledWith("eng");
  });

  it("a long list: a window of LEVEL_ROWS rows that follows the selection", async () => {
    const names = Array.from(
      { length: 200 },
      (_, i) => `d${String(i).padStart(3, "0")}`,
    );
    const { field, onJump } = renderSelect(() => listing(names), "d000");
    await screen.findAllByRole("option");
    expect(rows()).toHaveLength(LEVEL_ROWS);

    for (let i = 0; i < 150; i++)
      fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(selected()).toBe("d150");
    expect(rows()).toHaveLength(LEVEL_ROWS);
    expect(screen.getByText("151/200")).toBeInTheDocument();
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onJump).toHaveBeenLastCalledWith("d150");

    for (let i = 0; i < 100; i++)
      fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(selected()).toBe("d199");
    expect(rows().at(-1)).toBe("d199");
    expect(rows()).toHaveLength(LEVEL_ROWS);
    expect(screen.getByText("200/200")).toBeInTheDocument();
  });
});
