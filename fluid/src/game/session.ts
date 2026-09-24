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
 * (`loadPlace`), with the connector shown while it loads, and replaces the
 * room when it lands; `showCanned` shows a place that is already in hand (the
 * look demo's bridge) at once, and `showRoom` a room built by hand (the model
 * gallery). A room is generated once per entry and kept until the next one.
 *
 * Loads race, and the session settles every race the same way: each `go`
 * takes a new generation and aborts the load before it, and a load whose
 * generation is no longer the current one, or that settles after `dispose`,
 * changes nothing. A place that is missing, denied or unreachable leaves the
 * player where they stand with a notice for three seconds; with no room to
 * stand in yet, the notice stays over the dark screen.
 *
 * Each tick, in order: the look and command keys, movement, the doors, what
 * the player faces (the HUD prompt), E at it, the ways out of the room, and
 * warming the cache for the places behind the doors the player walks up to.
 *
 * Keys, by `KeyboardEvent.code`: W, A, S and D walk, the arrows turn and
 * look, E uses what the player faces, F opens the current engram in Fluid,
 * I inverts the vertical look (remembered in `localStorage` under
 * `INVERT_KEY`), and 1, 2 and 4 switch the look. While the CRT reader is
 * open it reads the keys itself: the session ignores its own commands and
 * all movement until the host calls `closeReader`.
 */

import type { QueryClient } from "@tanstack/react-query";

import { engramRoute } from "../paths";
import { createInput } from "./core/input";
import { createLoop, type Clock } from "./core/loop";
import { loadPlace, prefetchPlace, type LoadedPlace } from "./data/source";
import { backbufferSize } from "./device";
import { createContext } from "./gl/context";
import { gameEngramRoute, placeKeyOf } from "./paths";
import { createLights, type LightState } from "./render/lights";
import { LOOKS, lookForKey, type LookId } from "./render/looks";
import { createRenderer, type Renderer } from "./render/renderer";
import { NOT_FOUND, generateRoom } from "./world/generate";
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
  EYE_HEIGHT,
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
 * - `frame`: the frame time and the render targets' colour format.
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
 *   canned places (the look demo); without one every `go` answers
 *   `SIGNAL LOST`.
 * - `navigate`: replaces the URL with a game route once a place has loaded
 *   and the URL does not already show it.
 * - `openFluid`: opens a Fluid path (the F key) outside the game.
 * - `forceRgba8`: take the RGBA8 bloom path whatever the GPU offers.
 * - `createRenderer`: see `RendererFactory`.
 * - `initialLook`: the look to start in; `aperture` when not given.
 * - `clock`: the loop's clock; the browser's when not given. Tests crank it
 *   by hand.
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
}

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
 *   since there is no engram to read.
 * - `go`, `showCanned` and `showRoom` close an open CRT reader first.
 * - `closeReader` tells the session the CRT reader was closed, which gives
 *   it the keys back.
 * - `dispose` stops everything and frees the GPU objects. It takes the
 *   reader and the connector down; nothing is written to the HUD after it.
 * - `current` is the place the player is in, null before the first one.
 */
export interface Session {
  go(address: PlaceAddress, arrival?: Arrival | null): void;
  showCanned(place: PlaceInput): void;
  showRoom(room: RoomSpec): void;
  closeReader(): void;
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
  denied: "ACCESS DENIED",
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
  let doors = new Map<number, DoorState>();
  let doorOpen = new Map<string, number>();
  let player: Player | null = null;
  let previous: Player | null = null;
  let prefetched = new Set<string>();
  let latched: number | null = null;
  let readerOpen = false;
  let lastPrompt: string | null = null;

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

  const input = createInput(canvas);
  const onClick = () => {
    if (!readerOpen) input.requestLock();
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
    if (disposed) return;
    if (noticeTimer !== null) clearTimeout(noticeTimer);
    hud.notice(text);
    noticeTimer = setTimeout(() => {
      noticeTimer = null;
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
   * Enters a generated room: hands it to the renderer and resets everything
   * that belongs to the room before it. `next` is the place the room was
   * generated from, or null for a room built by hand (`showRoom`), whose
   * terminals then open no reader. `keep` keeps the player and the doors,
   * for the same place shown again.
   */
  const enter = (
    next: PlaceInput | null,
    built: RoomSpec,
    arrival: Arrival | null,
    keep: boolean,
  ): void => {
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
    }
    previous = player;
    prefetched = new Set();
    latched = null;
    placeNotice = null;
    showStanding();
    renderer?.setRoom(room, LOOKS[lookId]);
    showStatus();
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
      return;
    }
    enter(loaded.place, generateRoom(loaded.place), arrival, false);
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
   * reader, drops the load in flight (a new generation, the old one
   * aborted) and takes the connector down if it was up.
   */
  const leave = (): number => {
    closeReader();
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
    if (client === null) {
      fail(FAILED.offline);
      return;
    }
    const label = labelFor(address);
    loading = true;
    loadingLabel = label;
    hud.connector(true, label, lookId);
    const abort = new AbortController();
    controller = abort;
    loadPlace(client, address.domain, address.permalink, abort.signal).then(
      (loaded) => {
        settle(gen, address, arrival, loaded);
      },
      (error: unknown) => {
        if (disposed || gen !== generation) return;
        loading = false;
        controller = null;
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

  const showRoom = (built: RoomSpec) => {
    if (disposed) return;
    leave();
    enter(null, built, null, false);
  };

  const takeTravel = (travel: Travel) => {
    if (current === null) return;
    latched = travel.fixture;
    go(travel.address, { via: travel.via, from: current });
  };

  const openReader = (index: number) => {
    const fixture = room?.fixtures[index];
    if (fixture?.kind !== "terminal" || place === null) return;
    readerOpen = true;
    input.clear();
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
    hud.reader(null);
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
        if (room !== null) renderer.setRoom(room, LOOKS[lookId]);
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
    if (readerOpen) {
      // The reader reads W, S, F and Esc itself; none of them is ours.
      for (const code of COMMAND_KEYS) input.pressed(code);
      input.takeLook();
    } else {
      for (const code of LOOK_KEYS) {
        if (!input.pressed(code)) continue;
        const next = lookForKey(code);
        if (next !== null && next !== lookId) {
          lookId = next;
          if (room !== null) renderer?.setRoom(room, LOOKS[lookId]);
          if (loading) hud.connector(true, loadingLabel, lookId);
          showStatus();
        }
      }
      if (input.pressed("KeyI")) {
        inverted = !inverted;
        writeInverted(inverted);
        flash(inverted ? "LOOK INVERTED" : "LOOK NORMAL", LOOK_NOTICE_MS);
      }
      if (input.pressed("KeyF") && current !== null) {
        opts.openFluid(engramRoute(current.domain, current.permalink));
      }
    }
    if (room === null || player === null) return;

    const look = readerOpen || loading ? { dx: 0, dy: 0 } : input.takeLook();
    const still = readerOpen || loading;
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

    const focus = readerOpen ? null : focusOf(room, player, doors);
    let pressedDoor: number | null = null;
    if (!still && input.pressed("KeyE") && focus !== null) {
      if (focus.kind === "terminal") {
        openReader(focus.index);
      } else if (focus.kind === "door") {
        pressedDoor = focus.index;
      } else if (focus.kind === "hatch") {
        const travel = hatchTravel(room, focus.index);
        if (travel !== null) takeTravel(travel);
      }
    }
    doors = stepDoors(room, player, doors, pressedDoor);
    doorOpen = new Map();
    for (const [index, state] of doors)
      doorOpen.set(`door:${index}`, state.open);
    setPrompt(readerOpen || loading ? null : (focus?.prompt ?? null));

    if (!loading && !readerOpen) {
      const travel = travelOf(room, player, doors);
      if (travel === null) {
        latched = null;
      } else if (travel.fixture !== latched) {
        takeTravel(travel);
      }
    }

    if (client !== null) {
      for (const f of room.fixtures) {
        if ((f.kind !== "door" && f.kind !== "portal") || f.address === null) {
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
          hud.frame(
            `${colorFormat}  ${ms.toFixed(1)} MS  ${Math.round(1000 / ms)} FPS`,
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
    dispose() {
      if (disposed) return;
      // The host's overlays go down with the session, so a host that
      // outlives it (StrictMode's second mount) starts clean.
      hud.reader(null);
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
