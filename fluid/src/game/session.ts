/**
 * The session: one running station on one canvas.
 *
 * Everything that runs belongs to the session: the WebGL context and the
 * renderer on it, the input, the 35 Hz loop, the light specials, the doors
 * and the player. The React shell that hosts it (the game route, the look
 * demo) only hands it a canvas and a `HudSink` and tears it down with
 * `dispose`, so the shell renders once and the session writes the HUD
 * straight into the DOM.
 *
 * Places come in two ways. `go` loads one through Fluid's query cache
 * (`loadPlace`), or through the `load` seam when the session was given one,
 * with the connector shown while it loads, and replaces the room when it
 * lands; `showCanned` shows a place that is already in hand (the look demo's
 * bridge) at once, and `showRoom` a room built by hand (the model gallery).
 * A room is generated once per entry and kept until the next one.
 *
 * Loads race, and the session settles every race the same way: each `go`
 * takes a new generation and aborts the load before it, and a load whose
 * generation is no longer the current one, or that settles after `dispose`,
 * changes nothing. A place that is missing, denied or unreachable leaves the
 * player where they stand with a notice for three seconds; with no room to
 * stand in yet, the notice stays over the dark screen.
 *
 * Each tick, in order: the typed keys for the level cheat's word, the look
 * and command keys, movement, what the player
 * faces and E at it, the doors, the faults of the broken ways, the HUD
 * prompt, the ways out of the room, warming the cache for the places
 * behind the doors the player walks up to, the room's light specials and
 * the blink banks (`render/blink.ts`, H11). The blink state is made once
 * per session, not per room: a hero blinks the same way wherever it
 * stands. Its gains go to the renderer as `draw`'s sixth argument.
 *
 * Malfunctions belong to one visit of a room. A travel that settles as
 * missing (404) or denied (403) marks the way it went through as failed
 * (`failed`, the fixture index to its seal label): the way then says
 * `SEALED <label>`, heads shut, carries no one and runs its fault once by
 * itself (a door once it has shut), then stutters like every broken way
 * while the player stays near (`world/malfunction.ts`). Nothing else marks
 * a way: not `SIGNAL LOST`, not a thrown `?LOAD ERROR`, not a room the
 * renderer refuses, and not a `go` from outside, which went through no
 * way. A failure that settles after the player has left for another place
 * is dropped with its generation. The marks and the running faults are
 * kept while the same place is shown again (`showCanned`'s keep, a look
 * switch, a restored GPU context) and cleared on every fresh entry. Their
 * frames go to the renderer as `draw`'s fifth argument, never into the
 * door states that decide travel, and collision reads neither.
 *
 * Keys, by `KeyboardEvent.code`: W, A, S and D walk, the arrows turn and
 * look, E uses what the player faces, F opens the current engram in Fluid,
 * I inverts the vertical look (remembered in `localStorage` under
 * `INVERT_KEY`), and 1, 2 and 4 switch the look. While the CRT reader is
 * open it reads the keys itself: the session ignores its own commands and
 * all movement until the host calls `closeReader`.
 *
 * Typed with no pause longer than a second, `idclev` opens the level select
 * when the host passed `onLevels` (the game route): the word's I toggle is
 * taken back on the match, and its E is swallowed right after `I D C L`, so
 * the word never opens a terminal. While the select is open the session
 * reads no keys, exactly as with the CRT reader, until the host calls
 * `closeLevels` or `jump` goes somewhere.
 */

import type { QueryClient } from "@tanstack/react-query";

import { engramRoute } from "../paths";
import { createCheatReader } from "./core/cheat";
import { createInput } from "./core/input";
import { createLoop, type Clock } from "./core/loop";
import { loadPlace, prefetchPlace, type LoadedPlace } from "./data/source";
import { backbufferSize } from "./device";
import { createContext } from "./gl/context";
import { bridgeAddress, gameEngramRoute, placeKeyOf } from "./paths";
import { createBlink } from "./render/blink";
import { createLights, type LightState } from "./render/lights";
import { LOOKS, lookForKey, type LookId } from "./render/looks";
import { createRenderer, type Renderer } from "./render/renderer";
import { ACCESS_DENIED, NOT_FOUND, generateRoom } from "./world/generate";
import {
  arrivalSpawn,
  focusOf,
  hatchTravel,
  approaches,
  samePlace,
  stepDoors,
  travelOf,
  type Arrival,
  type DoorState,
  type Travel,
} from "./world/interact";
import {
  armFault,
  faultFrames,
  isBrokenWay,
  stepFaults,
  type Fault,
  type FaultFrame,
} from "./world/malfunction";
import {
  EYE_HEIGHT,
  MAX_PITCH,
  blockersFor,
  headBob,
  lookDelta,
  stepPlayer,
  type Player,
} from "./world/move";
import type { Box, PlaceAddress, PlaceInput, RoomSpec } from "./world/types";

export type { Arrival } from "./world/interact";

/**
 * What the CRT reader is opened with: the engram's title and markdown, the
 * section the terminal stands for (its `##` heading and how many sections
 * of the same heading come before it, or null for the top), and the look
 * the station was drawn in when it opened, which picks the reader's screen
 * (the C64's blue for `freescape`, phosphor green otherwise).
 */
export interface ReaderState {
  title: string;
  content: string;
  section: { heading: string; occurrence: number } | null;
  look: LookId;
}

/**
 * Where the session writes what the player sees besides the room.
 *
 * - `prompt`: the line for what the player faces (`E READ Scope`), null
 *   when nothing.
 * - `status`: the room, the look, its condition and the mouse hint.
 * - `frame`: the frame time and the render targets' colour format, with
 *   `BUILD <ms> MS` appended once a room has been built: the time the last
 *   `renderer.setRoom` call took.
 * - `notice`: a centred message over the canvas (`ACCESS DENIED`), null to
 *   hide it.
 * - `connector`: the travel overlay, shown while a place loads, with the
 *   destination's label and the look to draw it in.
 * - `reader`: the CRT reader's content while a terminal is read, null when
 *   it closes.
 *
 * Every writer may be called at any rate; the ones called every quarter
 * second are meant to write the DOM directly, not to render React.
 */
export interface HudSink {
  prompt(text: string | null): void;
  status(text: string): void;
  frame(text: string): void;
  notice(text: string | null): void;
  connector(active: boolean, label: string, look: LookId): void;
  reader(state: ReaderState | null): void;
}

/**
 * Makes the renderer for a canvas: a context and a renderer on it, with the
 * colour format its targets use, or null when the canvas gives no context.
 * It may throw when the context refuses the renderer. The default asks the
 * canvas for WebGL2 (`createContext`); tests inject a stub so the session
 * runs in jsdom without ever calling `getContext`.
 */
export type RendererFactory = (
  canvas: HTMLCanvasElement,
  options: { forceRgba8: boolean },
) => { renderer: Renderer; color: string } | null;

/**
 * What a session is started with.
 *
 * - `client`: Fluid's query client, or null when the session only shows
 *   canned places (the look demo); without one and without `load`, every
 *   `go` answers `SIGNAL LOST`.
 * - `navigate`: replaces the URL with a game route once a place has loaded
 *   and the URL does not already show it.
 * - `openFluid`: opens a Fluid path (the F key) outside the game.
 * - `forceRgba8`: take the RGBA8 bloom path whatever the GPU offers.
 * - `createRenderer`: see `RendererFactory`.
 * - `initialLook`: the look to start in; `aperture` when not given.
 * - `clock`: the loop's clock; the browser's when not given. Tests crank it
 *   by hand.
 * - `load`: loads a place in place of the query client. The gallery's
 *   `?fault=` answers every travel with a failure through it, and tests use
 *   it to fail a travel without the API. Without it, `go` loads through
 *   `client`, or says `SIGNAL LOST` when there is none.
 * - `onLevels`: the level cheat's switch and channel; only the game route
 *   passes it. See the member.
 */
export interface SessionOptions {
  canvas: HTMLCanvasElement;
  client: QueryClient | null;
  hud: HudSink;
  navigate(path: string): void;
  openFluid(path: string): void;
  forceRgba8: boolean;
  createRenderer?: RendererFactory;
  initialLook?: LookId;
  clock?: Clock;
  load?: PlaceLoader;
  /**
   * The level cheat's switch and channel (C8): with it, typing `idclev`
   * opens the level select and the session calls it with true; closing
   * the select (`closeLevels`, a `go`, `dispose`) calls it with false.
   * Without it the word is nothing but its letters: the look demo and
   * the model gallery never pass it.
   */
  onLevels?: (open: boolean) => void;
}

/**
 * Loads the place at `address` for a travel or a `go`, and settles as
 * `loadPlace` does: a place, or the reason there is none. It rejects with
 * an `AbortError` once `signal` aborts, and with anything else for a
 * failure the server did not explain (`?LOAD ERROR`). See
 * `SessionOptions.load`.
 */
export type PlaceLoader = (
  address: PlaceAddress,
  signal: AbortSignal,
) => Promise<LoadedPlace>;

/**
 * A running station.
 *
 * - `go` travels to a place: loads it and, once it lands, enters it, placed
 *   by `arrival` (see `arrivalSpawn`) or at the entrance.
 * - `showCanned` shows a place already in hand, with no load. Showing the
 *   place already shown again (the demo's R key) keeps the player where
 *   they stand and the doors as they are.
 * - `showRoom` shows a room built by hand, with no place behind it (the
 *   dev-only model gallery): no load, no navigation, the player at the
 *   room's entrance and every door shut. Its terminals open no reader,
 *   since there is no engram to read. `view`, when given, sets the
 *   player's pitch after entering, clamped to `MAX_PITCH` (C18, 2.6b): the
 *   dev seams' close curio framing (`spotView` in `dev/spots.ts`). It is
 *   for those dev seams only; every other caller omits it and keeps the
 *   entrance's own pitch of 0.
 * - `go`, `showCanned` and `showRoom` close an open CRT reader or level
 *   select first.
 * - `closeReader` tells the session the CRT reader was closed, which gives
 *   it the keys back.
 * - `jump` goes to a domain's bridge (`bridgeAddress`), the level select's
 *   jump: a `go` from outside, so the player enters at the entrance.
 * - `closeLevels` tells the session the level select was closed, which
 *   gives it the keys back.
 * - `dispose` stops everything and frees the GPU objects. It takes the
 *   reader and the connector down; nothing is written to the HUD after it.
 * - `current` is the place the player is in, null before the first one.
 */
export interface Session {
  go(address: PlaceAddress, arrival?: Arrival | null): void;
  showCanned(place: PlaceInput): void;
  showRoom(room: RoomSpec, view?: { pitch: number }): void;
  closeReader(): void;
  /** Goes to a domain's bridge (`bridgeAddress`): the level select's jump. */
  jump(domain: string): void;
  /** The level select was closed: gives the session the keys back. */
  closeLevels(): void;
  dispose(): void;
  readonly current: PlaceAddress | null;
}

/** Where the inverted-look choice is remembered: "1" inverted, "0" normal. */
export const INVERT_KEY = "station.invertY";

/** How long a failed load's notice stays up, in milliseconds. */
export const NOTICE_MS = 3000;

/** How long the inverted-look notice stays up, in milliseconds. */
export const LOOK_NOTICE_MS = 1500;

/** The notice for a refused or missing GPU. */
const NO_DEVICE = "?DEVICE NOT PRESENT ERROR";

/** What each failed answer tells the player. */
const FAILED: Record<Exclude<LoadedPlace["kind"], "place">, string> = {
  missing: NOT_FOUND,
  denied: ACCESS_DENIED,
  offline: "SIGNAL LOST",
};

/** The notice for a load that failed in a way the server did not explain. */
const LOAD_ERROR = "?LOAD ERROR";

/** How much one tick of an arrow key looks up or down, in mouse pixels. */
const ARROW_LOOK = 12;

/** The look keys, in the order they are read. */
const LOOK_KEYS = ["Digit1", "Digit2", "Digit4"] as const;

/** Every key that commands the session, drained while the reader is open. */
const COMMAND_KEYS = ["KeyE", "KeyF", "KeyI", ...LOOK_KEYS] as const;

/** The renderer on a real WebGL2 context. */
const defaultFactory: RendererFactory = (canvas, options) => {
  const context = createContext(canvas, options);
  if (context === null) return null;
  return {
    renderer: createRenderer(context.gl, context.caps),
    color: context.caps.color,
  };
};

/** Reads the inverted-look choice; anything but a readable "1" is normal. */
function readInverted(): boolean {
  try {
    return window.localStorage.getItem(INVERT_KEY) === "1";
  } catch {
    return false;
  }
}

/** Remembers the inverted-look choice, when the storage lets it. */
function writeInverted(inverted: boolean): void {
  try {
    window.localStorage.setItem(INVERT_KEY, inverted ? "1" : "0");
  } catch {
    // A private window or blocked storage: the choice lasts this session.
  }
}

/** Whether an error is the `AbortError` a cancelled load rejects with. */
function isAbort(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}

/**
 * Starts a session on `opts.canvas`. See `Session` for what it does and the
 * module doc for how. Nothing is shown until `go`, `showCanned` or
 * `showRoom`.
 */
export function createSession(opts: SessionOptions): Session {
  const { canvas, client, hud } = opts;
  const factory = opts.createRenderer ?? defaultFactory;
  const now = () => (opts.clock ?? performance).now();

  let disposed = false;
  let lookId: LookId = opts.initialLook ?? "aperture";
  let inverted = readInverted();
  let place: PlaceInput | null = null;
  let current: PlaceAddress | null = null;
  let room: RoomSpec | null = null;
  let blockers: Box[] = [];
  let lights: LightState | null = null;
  /** The blink banks' gains, one state for the whole session (H11). */
  const blink = createBlink();
  let doors = new Map<number, DoorState>();
  let doorOpen = new Map<string, number>();
  let player: Player | null = null;
  let previous: Player | null = null;
  let prefetched = new Set<string>();
  let latched: number | null = null;
  let readerOpen = false;
  /** Whether the level select is open (only with `onLevels`). */
  let levelsOpen = false;
  /** The level cheat's word, read only when the host passed `onLevels`. */
  const cheat = opts.onLevels === undefined ? null : createCheatReader();
  /** Ticks run so far: the clock the word's gap is counted on (C2). */
  let ticks = 0;
  /** Whether the timed notice up is the inverted-look one (C6). */
  let lookFlash = false;
  /** Whether an overlay (the CRT reader or the level select) has the keys. */
  const modal = () => readerOpen || levelsOpen;
  let lastPrompt: string | null = null;
  /** Ways whose travel failed this visit: fixture index to its seal label. */
  let failed = new Map<number, string>();
  /** The fault clock of every broken way this visit. */
  let faults = new Map<number, Fault>();
  /** The running faults' frames, as the renderer draws them. */
  let faultNow: ReadonlyMap<number, FaultFrame> = new Map();
  /** The travel in flight: its generation and the fixture it went through. */
  let travelling: { gen: number; fixture: number } | null = null;

  let generation = 0;
  let controller: AbortController | null = null;
  let loading = false;
  /** The connector's label while a load is in flight. */
  let loadingLabel = "";

  let renderer: Renderer | null = null;
  let colorFormat = "";
  let deviceNotice: string | null = null;
  let placeNotice: string | null = null;
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;

  let frameSum = 0;
  let frameCount = 0;
  let lastReport = now();
  const started = now();
  /** The last `renderer.setRoom` call's time, in ms; null before the first. */
  let lastBuildMs: number | null = null;

  const input = createInput(canvas);
  const onClick = () => {
    if (!modal()) input.requestLock();
  };
  canvas.addEventListener("click", onClick);

  // The notice shown when no timed one is up: a missing GPU wins over a
  // place that could not be entered.
  const standingNotice = () => deviceNotice ?? placeNotice;

  const showStanding = () => {
    if (disposed || noticeTimer !== null) return;
    hud.notice(standingNotice());
  };

  const flash = (text: string, ms: number) => {
    lookFlash = false;
    if (disposed) return;
    if (noticeTimer !== null) clearTimeout(noticeTimer);
    hud.notice(text);
    noticeTimer = setTimeout(() => {
      noticeTimer = null;
      lookFlash = false;
      showStanding();
    }, ms);
  };

  const showStatus = () => {
    if (disposed) return;
    const parts = [LOOKS[lookId].name.toUpperCase()];
    if (room !== null) {
      parts.unshift(room.title.toUpperCase());
      parts.push(room.condition.toUpperCase());
    }
    parts.push(
      input.locked ? "ESC RELEASES THE MOUSE" : "CLICK TO LOOK AROUND",
    );
    hud.status(parts.join("  |  "));
  };

  const setPrompt = (text: string | null) => {
    if (text === lastPrompt || disposed) return;
    lastPrompt = text;
    hud.prompt(text);
  };

  /**
   * The label the connector shows for a place: its title, when the room the
   * player is in knows it (a relation, a wikilink or an inbound reference
   * that leads there), else its permalink.
   */
  const labelFor = (address: PlaceAddress): string => {
    if (place !== null) {
      for (const r of [...place.relations, ...place.links]) {
        if (
          r.targetTitle !== null &&
          r.address !== null &&
          samePlace(r.address, address)
        ) {
          return r.targetTitle;
        }
      }
      for (const h of place.inbound) {
        if (samePlace(h.address, address)) return h.title;
      }
    }
    return address.permalink;
  };

  /**
   * Hands a room in a look to the renderer and says whether it took it. A
   * renderer that throws (a room that needs more texture layers than the
   * GPU holds, a mesh it cannot build) refuses the room; with no renderer
   * at all (no GPU yet, or a lost context) there is nothing to refuse, and
   * `boot` hands the room over when a renderer is made.
   */
  const present = (next: RoomSpec, id: LookId): boolean => {
    if (renderer === null) return true;
    const t0 = now();
    try {
      renderer.setRoom(next, LOOKS[id]);
      lastBuildMs = now() - t0;
      return true;
    } catch {
      return false;
    }
  };

  /**
   * Enters a generated room: hands it to the renderer and resets everything
   * that belongs to the room before it. `next` is the place the room was
   * generated from, or null for a room built by hand (`showRoom`), whose
   * terminals then open no reader. `keep` keeps the player, the doors and
   * this visit's failed ways and faults, for the same place shown again.
   *
   * The renderer is asked first. When it refuses the room, nothing of the
   * session has changed yet: the player stays in the room they were in,
   * with its blockers, lights and doors, and `fail` says `?LOAD ERROR`.
   * Returns whether the room was entered. Every press not yet consumed is
   * dropped on the way in, so a key hit for the room left behind (an E
   * while this one loaded) does nothing here.
   */
  const enter = (
    next: PlaceInput | null,
    built: RoomSpec,
    arrival: Arrival | null,
    keep: boolean,
  ): boolean => {
    if (!present(built, lookId)) {
      fail(LOAD_ERROR);
      return false;
    }
    input.dropPresses();
    place = next;
    room = built;
    blockers = blockersFor(room);
    lights = createLights(room.lights);
    current = { domain: built.domain, permalink: built.permalink };
    if (!keep || player === null) {
      const spawn = arrivalSpawn(room, arrival);
      player = { ...spawn, vx: 0, vz: 0, pitch: 0, bob: 0 };
      doors = new Map();
      doorOpen = new Map();
      failed = new Map();
      faults = new Map();
      faultNow = new Map();
    }
    previous = player;
    prefetched = new Set();
    latched = null;
    placeNotice = null;
    showStanding();
    showStatus();
    return true;
  };

  const settle = (
    gen: number,
    address: PlaceAddress,
    arrival: Arrival | null,
    loaded: LoadedPlace,
  ) => {
    if (disposed || gen !== generation) return;
    loading = false;
    controller = null;
    hud.connector(false, labelFor(address), lookId);
    if (loaded.kind !== "place") {
      fail(FAILED[loaded.kind]);
      // Only a missing or denied target breaks the way the travel went
      // through. A stale answer is dropped twice over: by the generation
      // guard above, and because `leave` clears `travelling` for every new
      // place, so `t` can only be this load's own travel (`t.gen === gen`).
      const t = travelling;
      travelling = null;
      if (
        t !== null &&
        t.gen === gen &&
        room !== null &&
        (loaded.kind === "missing" || loaded.kind === "denied")
      ) {
        failed = new Map(failed).set(t.fixture, FAILED[loaded.kind]);
        faults = armFault(room, t.fixture, faults);
      }
      return;
    }
    travelling = null;
    if (!enter(loaded.place, generateRoom(loaded.place), arrival, false)) {
      return;
    }
    const here = loaded.place;
    const path = gameEngramRoute(here.domain, here.permalink);
    if (window.location.pathname !== path) opts.navigate(path);
  };

  /**
   * A place that could not be entered: a notice for a while over the room
   * the player is still in, or for good over the dark screen before the
   * first room.
   */
  const fail = (text: string) => {
    if (room === null) {
      placeNotice = text;
      if (noticeTimer !== null) {
        clearTimeout(noticeTimer);
        noticeTimer = null;
      }
      showStanding();
    } else {
      flash(text, NOTICE_MS);
    }
  };

  /**
   * Leaves whatever the session was doing for a new place: closes the CRT
   * reader and the level select, drops the load in flight (a new generation, the old one
   * aborted) and takes the connector down if it was up.
   */
  const leave = (): number => {
    closeReader();
    closeLevels();
    travelling = null;
    const gen = ++generation;
    controller?.abort();
    controller = null;
    if (loading) {
      loading = false;
      hud.connector(false, loadingLabel, lookId);
    }
    return gen;
  };

  const go = (address: PlaceAddress, arrival: Arrival | null = null) => {
    if (disposed) return;
    const gen = leave();
    const loader: PlaceLoader | null =
      opts.load ??
      (client === null
        ? null
        : (a, signal) => loadPlace(client, a.domain, a.permalink, signal));
    if (loader === null) {
      fail(FAILED.offline);
      return;
    }
    const label = labelFor(address);
    loading = true;
    loadingLabel = label;
    hud.connector(true, label, lookId);
    const abort = new AbortController();
    controller = abort;
    loader(address, abort.signal).then(
      (loaded) => {
        settle(gen, address, arrival, loaded);
      },
      (error: unknown) => {
        if (disposed || gen !== generation) return;
        loading = false;
        controller = null;
        travelling = null;
        hud.connector(false, label, lookId);
        if (!isAbort(error)) fail(LOAD_ERROR);
      },
    );
  };

  const showCanned = (next: PlaceInput) => {
    if (disposed) return;
    leave();
    const same =
      current !== null &&
      samePlace(current, { domain: next.domain, permalink: next.permalink });
    enter(next, generateRoom(next), null, same);
  };

  const showRoom = (built: RoomSpec, view?: { pitch: number }) => {
    if (disposed) return;
    leave();
    if (!enter(null, built, null, false) || view === undefined) return;
    if (player === null) return;
    const pitch = Math.max(-MAX_PITCH, Math.min(MAX_PITCH, view.pitch));
    player = { ...player, pitch };
    previous = player;
  };

  const takeTravel = (travel: Travel) => {
    if (current === null) return;
    latched = travel.fixture;
    go(travel.address, { via: travel.via, from: current });
    // Only a travel the session took can mark its way failed (M2).
    if (loading) travelling = { gen: generation, fixture: travel.fixture };
  };

  const openReader = (index: number) => {
    const fixture = room?.fixtures[index];
    if (fixture?.kind !== "terminal" || place === null) return;
    readerOpen = true;
    input.clear();
    cheat?.reset();
    if (document.pointerLockElement !== null) {
      document.exitPointerLock?.();
    }
    setPrompt(null);
    hud.reader({
      title: place.title,
      content: place.content,
      section: { heading: fixture.heading, occurrence: fixture.section },
      look: lookId,
    });
  };

  const closeReader = () => {
    if (disposed || !readerOpen) return;
    readerOpen = false;
    input.clear();
    cheat?.reset();
    hud.reader(null);
  };

  /**
   * Opens the level select on a match of the word: takes the word's I
   * back (C6), gives the keys and the mouse to the overlay as the CRT
   * reader does, and tells the host.
   */
  const openLevels = () => {
    if (disposed || opts.onLevels === undefined) return;
    levelsOpen = true;
    cheat?.reset();
    input.clear();
    inverted = !inverted;
    writeInverted(inverted);
    if (lookFlash && noticeTimer !== null) {
      clearTimeout(noticeTimer);
      noticeTimer = null;
      lookFlash = false;
      showStanding();
    }
    if (document.pointerLockElement !== null) {
      document.exitPointerLock?.();
    }
    setPrompt(null);
    opts.onLevels(true);
  };

  const closeLevels = () => {
    if (disposed || !levelsOpen) return;
    levelsOpen = false;
    cheat?.reset();
    input.clear();
    opts.onLevels?.(false);
  };

  // Sizes the backbuffer to the canvas's CSS size times the pixel ratio and
  // says whether it changed, so a resize that changes nothing rebuilds no
  // render targets.
  const sizeCanvas = (): boolean => {
    const { width, height } = backbufferSize(
      canvas.clientWidth,
      canvas.clientHeight,
      window.devicePixelRatio,
      1,
    );
    if (canvas.width === width && canvas.height === height) return false;
    canvas.width = width;
    canvas.height = height;
    return true;
  };

  const resize = () => {
    if (sizeCanvas()) renderer?.resize(canvas.width, canvas.height);
  };

  // Dragging the window between displays changes only the pixel ratio,
  // which the ResizeObserver does not see; a resolution query for the
  // current ratio fires once when it stops matching and re-arms itself.
  let ratioQuery: MediaQueryList | null = null;
  const onRatioChange = () => {
    resize();
    watchRatio();
  };
  const watchRatio = () => {
    ratioQuery?.removeEventListener("change", onRatioChange);
    ratioQuery = null;
    if (typeof window.matchMedia !== "function") return;
    ratioQuery = window.matchMedia(
      `(resolution: ${window.devicePixelRatio}dppx)`,
    );
    ratioQuery.addEventListener("change", onRatioChange);
  };

  // A context that exists can still refuse the renderer, so a throw shows
  // the same notice as a missing context and leaves the loop stopped. A
  // fresh renderer has no targets yet, so its resize runs unconditionally.
  const boot = (): boolean => {
    let made: ReturnType<RendererFactory> = null;
    try {
      made = factory(canvas, { forceRgba8: opts.forceRgba8 });
      if (made !== null) {
        renderer = made.renderer;
        if (room !== null) {
          const t0 = now();
          renderer.setRoom(room, LOOKS[lookId]);
          lastBuildMs = now() - t0;
        }
        sizeCanvas();
        renderer.resize(canvas.width, canvas.height);
      }
    } catch {
      made?.renderer.dispose();
      made = null;
    }
    if (made === null) {
      renderer = null;
      deviceNotice = NO_DEVICE;
      showStanding();
      return false;
    }
    deviceNotice = null;
    showStanding();
    colorFormat = made.color.toUpperCase();
    hud.frame(colorFormat);
    return true;
  };

  const axis = (plus: string, minus: string) =>
    (input.held(plus) ? 1 : 0) - (input.held(minus) ? 1 : 0);

  const tick = () => {
    ticks++;
    // The keys typed since the last tick, in order: the level cheat's
    // word is read from them, and they are dropped unread while an
    // overlay has the keys (C4).
    const typed = input.typed();
    if (modal()) {
      // The reader and the level select read their keys themselves;
      // none of them is ours.
      for (const code of COMMAND_KEYS) input.pressed(code);
      input.takeLook();
    } else {
      let matched = false;
      if (cheat !== null) {
        for (const code of typed) {
          const step = cheat.feed(code, ticks);
          // The word's E is not a use: typed in front of a terminal it
          // never opens the reader (C7).
          if (step === "swallow") input.pressed("KeyE");
          if (step === "match") matched = true;
        }
      }
      // Nothing takes E while a place loads. Its press is dropped here
      // rather than kept for the room that loads or, when the load fails,
      // for this one.
      if (loading) input.pressed("KeyE");
      for (const code of LOOK_KEYS) {
        if (!input.pressed(code)) continue;
        const next = lookForKey(code);
        if (next === null || next === lookId) continue;
        // A look the renderer refuses the room in is not taken: the room
        // stays in the look it has.
        if (room !== null && !present(room, next)) {
          fail(LOAD_ERROR);
          continue;
        }
        lookId = next;
        if (loading) hud.connector(true, loadingLabel, lookId);
        showStatus();
      }
      if (input.pressed("KeyI")) {
        inverted = !inverted;
        writeInverted(inverted);
        flash(inverted ? "LOOK INVERTED" : "LOOK NORMAL", LOOK_NOTICE_MS);
        lookFlash = true;
      }
      if (input.pressed("KeyF") && current !== null) {
        opts.openFluid(engramRoute(current.domain, current.permalink));
      }
      if (matched) openLevels();
    }
    if (room === null || player === null) return;

    const look = modal() || loading ? { dx: 0, dy: 0 } : input.takeLook();
    const still = modal() || loading;
    previous = player;
    player = stepPlayer(
      player,
      {
        forward: still ? 0 : axis("KeyW", "KeyS"),
        strafe: still ? 0 : axis("KeyD", "KeyA"),
        turn: still ? 0 : axis("ArrowLeft", "ArrowRight"),
        lookDx: look.dx,
        lookDy: lookDelta(
          look.dy + (still ? 0 : axis("ArrowDown", "ArrowUp") * ARROW_LOOK),
          inverted,
        ),
      },
      room,
      blockers,
    );

    const focus = modal() ? null : focusOf(room, player, doors, failed);
    let pressedDoor: number | null = null;
    let pressedWay: number | null = null;
    if (!still && input.pressed("KeyE") && focus !== null) {
      if (isBrokenWay(room, focus.index, failed)) pressedWay = focus.index;
      if (focus.kind === "terminal") {
        openReader(focus.index);
      } else if (focus.kind === "door") {
        pressedDoor = focus.index;
      } else if (focus.kind === "hatch") {
        // A failed hatch carries no one: `hatchTravel` reads `failed`.
        const travel = hatchTravel(room, focus.index, failed);
        if (travel !== null) takeTravel(travel);
      }
    }
    doors = stepDoors(room, player, doors, pressedDoor, failed);
    faults = stepFaults(room, player, faults, failed, pressedWay, doors);
    faultNow = faultFrames(faults);
    doorOpen = new Map();
    for (const [index, state] of doors)
      doorOpen.set(`door:${index}`, state.open);
    setPrompt(modal() || loading ? null : (focus?.prompt ?? null));

    if (!loading && !modal()) {
      const travel = travelOf(room, player, doors, failed);
      if (travel === null) {
        latched = null;
      } else if (travel.fixture !== latched) {
        takeTravel(travel);
      }
    }

    if (client !== null) {
      for (const [index, f] of room.fixtures.entries()) {
        if (
          (f.kind !== "door" && f.kind !== "portal") ||
          f.address === null ||
          failed.has(index)
        ) {
          continue;
        }
        const key = placeKeyOf(f.address.domain, f.address.permalink);
        if (prefetched.has(key)) continue;
        if (approaches(f.slot, player)) {
          prefetched.add(key);
          prefetchPlace(client, f.address.domain, f.address.permalink);
        }
      }
    }
    lights?.tick();
    blink.tick();
  };

  const loop = createLoop(
    {
      tick,
      render(alpha, frameMs) {
        frameSum += frameMs;
        frameCount++;
        const t = now();
        if (t - lastReport > 250) {
          const ms = frameSum / frameCount;
          const build =
            lastBuildMs === null ? "" : `  BUILD ${lastBuildMs.toFixed(1)} MS`;
          hud.frame(
            `${colorFormat}  ${ms.toFixed(1)} MS  ${Math.round(1000 / ms)} FPS${build}`,
          );
          frameSum = 0;
          frameCount = 0;
          lastReport = t;
          showStatus();
        }
        if (player === null || previous === null || lights === null) return;
        const lerp = (a: number, b: number) => a + (b - a) * alpha;
        renderer?.draw(
          {
            eye: [
              lerp(previous.x, player.x),
              EYE_HEIGHT + headBob(player),
              lerp(previous.z, player.z),
            ],
            yaw: lerp(previous.yaw, player.yaw),
            pitch: lerp(previous.pitch, player.pitch),
          },
          lights.levels,
          (t - started) / 1000,
          doorOpen,
          faultNow,
          blink.gains,
        );
      },
    },
    opts.clock,
  );

  const observer =
    typeof ResizeObserver === "function" ? new ResizeObserver(resize) : null;
  observer?.observe(canvas);
  watchRatio();
  const onLost = (e: Event) => {
    e.preventDefault();
    loop.stop();
    renderer = null;
    deviceNotice = "SIGNAL LOST - WAITING FOR THE GPU";
    showStanding();
  };
  const onRestored = () => {
    if (boot()) loop.start();
  };
  canvas.addEventListener("webglcontextlost", onLost);
  canvas.addEventListener("webglcontextrestored", onRestored);

  if (boot()) loop.start();
  showStatus();

  return {
    go,
    showCanned,
    showRoom,
    closeReader,
    jump(domain) {
      go(bridgeAddress(domain));
    },
    closeLevels,
    dispose() {
      if (disposed) return;
      // The host's overlays go down with the session, so a host that
      // outlives it (StrictMode's second mount) starts clean.
      hud.reader(null);
      opts.onLevels?.(false);
      hud.connector(false, loadingLabel, lookId);
      disposed = true;
      generation++;
      controller?.abort();
      controller = null;
      if (noticeTimer !== null) clearTimeout(noticeTimer);
      noticeTimer = null;
      loop.stop();
      observer?.disconnect();
      ratioQuery?.removeEventListener("change", onRatioChange);
      ratioQuery = null;
      canvas.removeEventListener("click", onClick);
      canvas.removeEventListener("webglcontextlost", onLost);
      canvas.removeEventListener("webglcontextrestored", onRestored);
      input.dispose();
      renderer?.dispose();
      renderer = null;
    },
    get current() {
      return current;
    },
  };
}
