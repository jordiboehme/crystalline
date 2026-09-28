/**
 * The C64 screen behind the gem, and its way into the station (M4 C1 to
 * C4).
 *
 * Mounted in the whole app on an in-memory history, the composition
 * `renderApp` builds (`MemoryRouter` around `App`) with two probes beside it
 * that read and move the router's location, the way `GameRoute.test.tsx`
 * does: the header, the router and the signed-in shell are real, so the
 * triple click's first two clicks really navigate home, and a launch really
 * lands on the `/π` route. That route's module is replaced by a marker, so
 * no station code runs here.
 */

import {
  fireEvent,
  getDefaultNormalizer,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { act, useEffect } from "react";
import {
  MemoryRouter,
  useLocation,
  useNavigate,
  type NavigateFunction,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "../App";
import { api } from "../api/client";
import { releasePrimedAudio } from "../game/launch";
import {
  answersFor,
  domainsResponse,
  meResponse,
  userFixture,
} from "../test/harness";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

vi.mock("../game/GameRoute", () => ({
  default: () => <p>station marker</p>,
}));

const apiMock = vi.mocked(api);

/** The router's pathname and search, as the probe last saw them. */
let location = "";
let navigate: NavigateFunction | null = null;

/** Records the router's pathname and search each time they change. */
function LocationProbe() {
  const { pathname, search } = useLocation();
  useEffect(() => {
    location = pathname + search;
  }, [pathname, search]);
  return null;
}

/** Hands the test the router's own `navigate`. */
function NavProbe() {
  const nav = useNavigate();
  useEffect(() => {
    navigate = nav;
  }, [nav]);
  return null;
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
      <LocationProbe />
      <NavProbe />
    </MemoryRouter>,
  );
}

/** Moves the router, as a link or the history buttons would. */
async function go(to: string) {
  await act(async () => {
    await navigate?.(to);
  });
}

beforeEach(() => {
  apiMock.mockReset();
  localStorage.clear();
  location = "";
  navigate = null;
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
    }),
  );
});

afterEach(() => {
  releasePrimedAudio();
});

/**
 * Triple-clicks the gem in the header's home link, the way a browser sends
 * it: three clicks with `detail` 1, 2 and 3, the first two of which the
 * home link follows. Resolves with the screen.
 */
async function openScreen(): Promise<HTMLElement> {
  const home = await screen.findByRole("link", { name: "Fluid" });
  const gem = home.firstElementChild;
  if (!(gem instanceof HTMLElement)) throw new Error("no gem in the home link");
  fireEvent.click(gem, { detail: 1 });
  fireEvent.click(gem, { detail: 2 });
  fireEvent.click(gem, { detail: 3 });
  return screen.findByRole("dialog", { name: "About Crystalline" });
}

/** The screen's cursor line, the typed text without the cursor block. */
function cursorLine(dialog: HTMLElement): string {
  const cursor = dialog.querySelector(".crystal-cursor");
  const line = cursor?.parentElement;
  if (!line) throw new Error("no cursor line");
  return (line.textContent ?? "").replace("█", "");
}

/** The element a typed key is sent to: whatever has the focus. */
function focused(): Element {
  return document.activeElement ?? document.body;
}

/** Types `text` key by key at `target`, the `"` as a German layout sends it. */
function type(text: string, target: () => Element = focused) {
  for (const key of text) {
    fireEvent.keyDown(
      target(),
      key === '"' ? { key, code: "Digit2", shiftKey: true } : { key },
    );
  }
}

/** `?SYNTAX  ERROR` with its two spaces, which the default matcher collapses. */
function syntaxErrors(dialog: HTMLElement): HTMLElement[] {
  return within(dialog).queryAllByText("?SYNTAX  ERROR", {
    normalizer: getDefaultNormalizer({ collapseWhitespace: false }),
  });
}

describe("the C64 screen's launch (M4 C1 to C4)", () => {
  it("launches from the page the first click came from (Review Focus 1)", async () => {
    // Mutation caught: the origin read at the third click (always "/"), the
    // origin kept from an earlier visit.
    renderAt("/d/eng/e/notes/deep/gamma");
    let dialog = await openScreen();
    // The first two clicks followed the home link.
    expect(location).toBe("/");
    const game = within(dialog).getByRole("link", { name: 'LOAD"GAME",8,1' });
    expect(game).toHaveAttribute("href", "/%CF%80/d/eng/e/notes/deep/gamma");

    // Closed and opened again from another page in the same app: the
    // origin is the new page, not the one remembered from before.
    fireEvent.keyDown(focused(), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await go("/d/eng?path=My%20Notes");
    expect(location).toBe("/d/eng?path=My%20Notes");
    dialog = await openScreen();
    fireEvent.click(
      within(dialog).getByRole("link", { name: 'LOAD"GAME",8,1' }),
    );
    await screen.findByText("station marker");
    expect(location).toBe("/%CF%80/d/eng?path=My%20Notes");
  });

  it("remembers the origin when a long press opens the screen", async () => {
    // Mutation caught: the long press not setting the origin (its trailing
    // click is suppressed before the first-click branch), so a touch launch
    // starts from the page an earlier triple click remembered.
    renderAt("/d/eng/e/notes/deep/gamma");
    await openScreen();
    fireEvent.keyDown(focused(), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await go("/d/eng?path=My%20Notes");
    const home = await screen.findByRole("link", { name: "Fluid" });
    const gem = home.firstElementChild;
    if (!(gem instanceof HTMLElement))
      throw new Error("no gem in the home link");
    fireEvent.pointerDown(gem, { pointerType: "touch" });
    const dialog = await screen.findByRole(
      "dialog",
      { name: "About Crystalline" },
      { timeout: 3000 },
    );
    expect(
      within(dialog).getByRole("link", { name: 'LOAD"GAME",8,1' }),
    ).toHaveAttribute("href", "/%CF%80/d/eng?path=My%20Notes");
  });

  it("launches from the page it was opened on with a single launch", async () => {
    // Mutation caught: the Link's target left at the current location
    // ("/" after the home link's navigation) instead of the origin.
    renderAt("/d/eng/e/notes/deep/gamma");
    const dialog = await openScreen();
    fireEvent.click(
      within(dialog).getByRole("link", { name: 'LOAD"GAME",8,1' }),
    );
    await screen.findByText("station marker");
    expect(location).toBe("/%CF%80/d/eng/e/notes/deep/gamma");
  });

  it("reads the typed command by key, in any case and spacing (Review Focus 1)", async () => {
    // Mutation caught: matching `code` (a German `"` is Digit2), a
    // case-sensitive compare, spaces not removed, Enter on an empty line
    // acting, a modifier key typed.
    renderAt("/search?q=x");
    let dialog = await openScreen();
    type('load "game", 8, 1');
    expect(cursorLine(dialog)).toBe('LOAD "GAME", 8, 1');
    fireEvent.keyDown(focused(), { key: "Enter" });
    await screen.findByText("station marker");
    expect(location).toBe("/%CF%80/search?q=x");

    await go("/");
    await screen.findByRole("main");
    dialog = await openScreen();

    // Enter on an empty line does nothing and is left to the browser.
    expect(fireEvent.keyDown(focused(), { key: "Enter" })).toBe(true);
    expect(syntaxErrors(dialog)).toHaveLength(0);
    expect(location).toBe("/");

    // A modified key types nothing and is left to whoever owns it.
    expect(fireEvent.keyDown(focused(), { key: "l", ctrlKey: true })).toBe(
      true,
    );
    expect(fireEvent.keyDown(focused(), { key: "l", metaKey: true })).toBe(
      true,
    );
    expect(fireEvent.keyDown(focused(), { key: "l", altKey: true })).toBe(true);
    expect(cursorLine(dialog)).toBe("");

    // Backspace takes the last key back; a typo is a syntax error.
    type("LOAD");
    fireEvent.keyDown(focused(), { key: "Backspace" });
    expect(cursorLine(dialog)).toBe("LOA");
    fireEvent.keyDown(focused(), { key: "Backspace" });
    fireEvent.keyDown(focused(), { key: "Backspace" });
    fireEvent.keyDown(focused(), { key: "Backspace" });
    type('LOAD"GANE",8,1');
    expect(fireEvent.keyDown(focused(), { key: "Enter" })).toBe(false);
    expect(syntaxErrors(dialog)).toHaveLength(1);
    expect(cursorLine(dialog)).toBe("");
    expect(location).toBe("/");
    expect(screen.queryByText("station marker")).toBeNull();
  });

  it("caps the typed line at 40 characters", async () => {
    // Mutation caught: the cap dropped or set off by one.
    renderAt("/");
    const dialog = await openScreen();
    type("X".repeat(45));
    expect(cursorLine(dialog)).toBe("X".repeat(40));
  });

  it("keeps the keys it types from the app's own shortcuts (F27, F42)", async () => {
    // Mutation caught: the listener in the bubble phase (a typed `?` opens
    // the help dialog behind the screen, a typed `\` toggles the width),
    // Enter on a focused LOAD link with a typed line following the link.
    renderAt("/d/eng/e/notes/deep/gamma");
    const dialog = await openScreen();
    fireEvent.keyDown(focused(), { key: "?", shiftKey: true });
    fireEvent.keyDown(focused(), { key: "\\" });
    expect(cursorLine(dialog)).toBe("?\\");
    // The help overlay's body is a lazy chunk: settle its import, so a help
    // dialog the `?` opened would be drawn by now.
    await act(async () => {
      await import("./HelpOverlayBody");
    });
    expect(
      screen.queryByRole("dialog", { name: /keyboard shortcuts/i }),
    ).toBeNull();
    expect(document.querySelector("main")).not.toHaveAttribute("data-width");

    fireEvent.keyDown(focused(), { key: "Backspace" });
    fireEvent.keyDown(focused(), { key: "Backspace" });
    const source = within(dialog).getByRole("link", {
      name: 'LOAD"SOURCE",8,1',
    });
    source.focus();
    expect(document.activeElement).toBe(source);
    type('LOAD"GAME",8,1', () => source);
    // The Enter's default, following the focused link, is prevented.
    expect(fireEvent.keyDown(source, { key: "Enter" })).toBe(false);
    await screen.findByText("station marker");
    expect(location).toBe("/%CF%80/d/eng/e/notes/deep/gamma");
  });

  it("primes sound inside the launching gesture", async () => {
    // Mutation caught: primeAudio called after navigate (outside the gesture)
    // or not at all.
    let inside = false;
    const made: { inside: boolean; location: string }[] = [];
    class StubContext {
      state = "suspended";
      constructor() {
        made.push({ inside, location });
      }
      resume() {
        return Promise.resolve();
      }
      close() {
        return Promise.resolve();
      }
    }
    vi.stubGlobal("AudioContext", StubContext);
    try {
      renderAt("/d/eng/e/notes/deep/gamma");
      let dialog = await openScreen();
      const game = within(dialog).getByRole("link", {
        name: 'LOAD"GAME",8,1',
      });
      inside = true;
      fireEvent.click(game);
      inside = false;
      await screen.findByText("station marker");
      expect(made).toEqual([{ inside: true, location: "/" }]);

      // And the typed command, inside its Enter.
      await go("/");
      await screen.findByRole("main");
      dialog = await openScreen();
      type('LOAD"GAME",8,1');
      inside = true;
      fireEvent.keyDown(focused(), { key: "Enter" });
      inside = false;
      await screen.findByText("station marker");
      expect(made).toEqual([
        { inside: true, location: "/" },
        { inside: true, location: "/" },
      ]);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("still closes on Esc and RUN/STOP, and lists the three lines", async () => {
    // Mutation caught: the typed handler swallowing Esc. The two links and
    // the new one are listed in order SOURCE, COFFEE, GAME.
    renderAt("/");
    let dialog = await openScreen();
    expect(
      within(dialog)
        .getAllByRole("link")
        .map((link) => link.textContent),
    ).toEqual(['LOAD"SOURCE",8,1', 'LOAD"COFFEE",8,1', 'LOAD"GAME",8,1']);
    type("AB");
    expect(fireEvent.keyDown(focused(), { key: "Escape" })).toBe(true);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    dialog = await openScreen();
    fireEvent.click(
      within(dialog).getByRole("button", { name: "RUN/STOP (ESC)" }),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(location).toBe("/");
  });
});
