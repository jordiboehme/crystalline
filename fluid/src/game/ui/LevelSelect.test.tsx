/**
 * The level select, rendered over a real query client with only `api`
 * stubbed: the listing, the filter, the selection, the keys, and the
 * listing that is still loading or failed.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../../api/client";
import { answersFor, type Answer } from "../../test/harness";
import { LevelSelect } from "./LevelSelect";
import {
  LEVEL_ROWS,
  LEVELS_FAILED,
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
  return { onJump, onClose, field };
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

  it("jumps on a click, and a mousedown keeps the focus in the field", async () => {
    const { field, onJump } = renderSelect(() => listing(["a", "b"]));
    await screen.findAllByRole("option");
    const row = screen.getAllByRole("option")[1]!;
    expect(fireEvent.mouseDown(row)).toBe(false);
    fireEvent.click(row);
    expect(onJump).toHaveBeenCalledWith("b");
    expect(field).toHaveFocus();
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
