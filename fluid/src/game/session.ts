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
 * The session speaks station addresses (M3 C1): the airlock, a domain's
 * bridge, a deck and an engram's room. Places come in two ways. `go` loads
 * one through Fluid's query cache (`loadStation`), or through the `load`
 * seam when the session was given one, with the connector shown while it
 * loads, and replaces the room when it lands: `roomFor` builds the room and
 * resolves the address entered (a deck's section), which becomes `current`
 * and whose game route replaces the URL unless the location's pathname and
 * search already spell it (C5). `showCanned` shows a place that is already
 * in hand (the look demo's bridge) at once, at its engram address, and
 * `showRoom` a room built by hand (the model gallery), at no address. A
 * room is generated once per entry and kept until the next one.
 *
 * Loads race, and the session settles every race the same way: each `go`
 * takes a new generation and aborts the load before it, and a load whose
 * generation is no longer the current one, or that settles after `dispose`,
 * changes nothing. A place that is missing, denied or unreachable leaves the
 * player where they stand with a notice for three seconds; with no room to
 * stand in yet, the notice stays over the dark screen.
 *
 * Each tick, in order: the typed keys for the level cheat's word, the
 * command keys, movement, what the player faces and a use of it, the doors,
 * the faults of the broken ways, the walk into a police box and out of the
 * console room, the HUD prompt, the ways out of the room,
 * warming the cache for the places behind the doors the player walks up to,
 * the room's light specials and the blink banks (`render/blink.ts`, H11).
 * The blink state is made once per session, not per room: a hero blinks the
 * same way wherever it stands. Its gains go to the renderer as `draw`'s
 * sixth argument.
 *
 * Space at a police box's front opens or closes its doors too
 * (`world/box.ts`'s `boxFocus` and `stepBoxDoors`), like a bulkhead door: a
 * fixture in focus always wins over a box, since the two rarely compete. Its
 * door state is kept under `boxKey(index)`, alongside the fixture doors'
 * `door:<index>` keys, in the same map the renderer draws every mover's
 * fraction from.
 *
 * With `SessionOptions.consoleRoom` (the game route only), walking through
 * a box's open doors (`boxEntry`) cuts into the console room
 * (`world/consoleRoom.ts`): instantly, with no connector, no load and no
 * navigation. The session enters the console room under the domain of the
 * room left and that room's own key (its `permalink`), and keeps the
 * address walked in from as `current`, so F and the URL keep naming that
 * room, and a reload inside returns to it. The cut in starts reading the
 * domain listing. Walking into the console room's inner doors
 * (`atConsoleExit`) travels to the bridge of a domain picked from that
 * listing (`pickExitDomain`, seeded by the tick count), with the connector
 * naming the domain; the exit waits while the listing is still being read,
 * and falls back to the room left's own domain when it could not be read
 * or has not answered within `LISTING_WAIT_MS`.
 * The bridge lands with a police box standing free near its entrance for
 * that visit (`withArrivalBox`): the player steps out of it, its doors open
 * and swinging shut, and its walk-in latched until the player has stepped
 * `BOX_LATCH_CLEAR` (1.2 m) away from its front (`steppedAway`). The
 * walk-in is latched that way after every walk-in, the exit like a way, so
 * each fires again only after the player has stepped away; a failed exit leaves
 * the player inside with the notice. Neither fires while a load is in flight
 * or an overlay has the keys, and every entry of a room ends the visit of
 * the console room (a jump from inside leaves it like any `go`).
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
 * kept while the same place is shown again (`showCanned`'s keep, a
 * restored GPU context) and cleared on every fresh entry. Their
 * frames go to the renderer as `draw`'s fifth argument, never into the
 * door states that decide travel, and collision reads neither.
 *
 * Keys, by `KeyboardEvent.code`, the classic layout plus WASD: Up and Down
 * (or W and S) walk, Left and Right turn, and strafe while Alt is held,
 * comma and period (or A and D) strafe, Shift held runs, Space uses what
 * the player faces, F opens the page of `page` in Fluid
 * (`fluidRouteOfStation`: `/` in the airlock, the domain page on a bridge,
 * the folder on a deck, the engram in its room), I inverts the
 * mouse's vertical look (remembered in `localStorage` under `INVERT_KEY`),
 * and M turns the sound off and on (with `sound`: its `toggleMute`, the
 * notice `SOUND OFF` or `SOUND ON`; without it M does nothing).
 * No key switches the look: the game always runs in Aperture grid (look
 * 2), and only the dev pages start in another (`initialLook`, their
 * `?look=`). Only the mouse looks up and down. The
 * browser's own meaning of Space, the arrows, comma, period and Alt is
 * cancelled while the session has the keys (`CLAIMED_KEYS`). While the CRT
 * reader is open it reads the keys itself: the session ignores its own
 * commands and all movement until the host calls `closeReader`.
 *
 * Space at a lift (M3 C26) opens its stops when the host passed `onLift`
 * (the game route): the session is modal while they are open, as with the
 * level select, until the host calls `closeLift` or `ride`. A lift that
 * reads `?DOMAIN LIST ERROR` (the airlock whose listing failed, C29) opens
 * nothing and reads the airlock again instead. `ride` travels to a stop
 * with the connector naming it and holds a settled load until
 * `LIFT_RIDE_MS` after the ride began (C27), a check made each tick, so
 * a quick load still shows the ride; the player comes out at the
 * entrance, in front of the lift, its doors open and sliding shut. An
 * engram room's exit (C28) is walked through like a sliding door and
 * leads up to its deck, in front of the deck's door back to the room.
 * It is latched on every entry (`upLatched`): the entrance spawn stands
 * 1 m from it, so it stays shut and carries no one until the player has
 * once stood `UP_LATCH_CLEAR` from its wall. A failed stop or exit is a
 * notice only: neither marks a way (C29).
 *
 * Typed with no pause longer than a second, `idclev` opens the level select
 * when the host passed `onLevels` (the game route): the word's I toggle is
 * taken back on the match, and its D strafes for a moment; its E is a plain
 * letter, since Space is the use key. While the select is open the session
 * reads no keys, exactly as with the CRT reader, until the host calls
 * `closeLevels` or `jump` goes somewhere.
 *
 * With `onPause` (the game route only), the station pauses (M4 C6) when
 * the pointer lock ends while nothing else has the keys, and on a fresh
 * Esc while the pointer is not locked (the path a browser that refuses
 * the lock takes), unless something else already took that Esc
 * (`defaultPrevented`). The session listens for both
 * itself, beside the input's own listeners, and registers them when it is
 * created, before any overlay mounts, so an overlay's Esc finds the
 * session modal (C9). A load in flight does not stop a pause; it settles
 * under it. While paused the session is modal: the tick drains the keys
 * and returns, so nothing moves, no door, fault or light steps and nothing
 * is travelled to; nothing is drawn under the host's screen either, and
 * the shader's clock holds still, so no effect jumps on resume. The host
 * shows its pause screen from `onPause` and ends the pause with `resume`,
 * in the gesture that asks for the lock again; a new place (`leave`) ends
 * it too.
 * `setBusy` holds the station modal the same way for a screen of the
 * host's (the connecting screen), without a pause. Going down pauses
 * nothing: both listeners are removed before the input releases the lock.
 *
 * Live changes (M4 C14 to C19) come in through `changed`, which the host
 * calls for every frame of the change stream after it made the frame's
 * keys stale. A `moved` frame of the current engram sets the address the
 * room follows to (`movedTo`, C18). A frame that concerns the place
 * (`concerns` on `watchOf`: an engram room watches its domain and the
 * domains its ways lead to, a bridge or a deck its domain, the airlock
 * the listing, the console room nothing) opens a `COALESCE_MS` window;
 * frames inside it join it, and at its end one re-check runs. A re-check
 * waits, owed (`pendingCheck`, the latest wins), while a load or a lift
 * ride is in flight, an overlay is open, the station is paused or busy,
 * a flicker or a dip is running, or another re-check is loading, and
 * runs once that ends. It loads the current address again (or the one a
 * move sent it to) through the session's loader, builds the room with
 * the arrival and the landing the room was entered with, and diffs it
 * with the room shown (`diffRooms`, C16): `same` does nothing; `text`
 * re-enters the room keeping the player, the doors, the malfunctions and
 * the latches, and flickers the lights for `FLICKER_MS`; `shape` shows
 * `STATION RECONFIGURING`, dips the lights for `DIP_MS` and enters the
 * rebuilt room at `DIP_SWAP_MS`, the player where they stood or on the
 * nearest free floor (`settleSpot`), their view kept, the doors shut and
 * the malfunctions cleared. A shape change within `RESHAPE_MIN_MS` of the
 * last is held until that has passed. A move replaces the URL once its
 * room is entered. A room whose engram went (a 404) or is refused (a 403)
 * goes dark in place (`darkened`, C19): the player kept (moved to the
 * nearest free floor when the dark room's props now stand there), the lights at a
 * quarter and failing, a hatch to the bridge, the notice standing, the
 * URL kept; a second such answer changes nothing, and a later one that
 * loads the room brings it back through a dip. The airlock is never
 * darkened. An `offline` answer flashes `SIGNAL LOST` and keeps the room.
 * A new place drops the re-check in flight, a running dip with its
 * notice, and a move not yet followed.
 *
 * Nothing changes under the pause: a re-check owed waits for `resume`, an
 * answer that lands while paused is dropped and the re-check owed again,
 * and a flicker or a dip holds still with the shader's clock and goes on
 * after it. So the room the pause screen names is the one the player sees
 * on continuing; a load of the player's own that lands under the pause
 * enters its room, and the host renames the screen through `navigate`.
 *
 * Sound (M4 C20 to C23): with `SessionOptions.sound`, the session sends a
 * cue (`audio/cues.ts`) for every moment that makes a sound, as it writes
 * the HUD, and never plays anything itself. `room` on every entry,
 * with the room's ambience (`ambienceOf`, the dark room's once it goes
 * dark) and seed: a load landing, a canned or hand-built room shown, the
 * cut into the console room, a re-check's re-entry or swap. `step` for
 * each footstep the tick's stride makes (`stepsBetween`), numbered through
 * the session, feet alternating, only while the player may move (not
 * while anything is modal or a place loads). `door` for every door, exit
 * or police box whose target turned (`doorCues`), after the doors
 * and the boxes stepped. `fault` for every broken way that started a run
 * (`faultCues`), after the faults stepped. `travel` for every way taken
 * (a door, a portal, a hatch, an exit) and a lift ride. `terminal` when
 * the reader, the level select or the lift's stops open, and on a text
 * change's re-entry. `ride` `depart` as a ride starts, and `arrive` once
 * as it ends, however it ends (its landing, held or not, a failed stop, a
 * load error or a new place), so the ride's hum never outlasts it. `box`
 * `takeoff` on the cut into the console room, and `landing` when the
 * console room's exit lands on a bridge with the arrival box standing.
 * `jump` on the level select's jump. `answer` from an answering console
 * (`answers`, M4 C25): once per visit, after the player has faced it
 * (`focusOf`, never while modal) for `ANSWER_WAIT_MS` without looking
 * away, placed at its wall point, never while a place loads; every entry
 * starts the visit over, and so do the pause and a hidden tab. Nothing is
 * sent while paused (the tick returns first) or after `dispose`;
 * suspending the sound under the pause is the host's, which hears of it
 * through `onPause`.
 */

import type { QueryClient } from "@tanstack/react-query";

import type { ChangeEvent } from "../api/events";
import { COALESCE_MS } from "../events/ChangeStreamProvider";
import {
  ambienceOf,
  doorCues,
  faultCues,
  placeCue,
  stepsBetween,
  type Cue,
  type SoundSink,
} from "./audio/cues";
import { ANSWER_WAIT_MS, answers } from "./audio/signature";
import { createCheatReader } from "./core/cheat";
import { createInput } from "./core/input";
import { createLoop, type Clock } from "./core/loop";
import { concerns, movedTo, watchOf } from "./data/changes";
import { prefetchPlace } from "./data/source";
import { loadStation, type LoadedStation } from "./data/station";
import { backbufferSize } from "./device";
import { createContext } from "./gl/context";
import {
  bridgeAddress,
  canonicalStation,
  domainOf,
  fluidRouteOfStation,
  gameRouteOf,
  placeKeyOf,
  sameStation,
  stationOfPlace,
} from "./paths";
import { createBlink } from "./render/blink";
import { createLights, type LightState } from "./render/lights";
import { LOOKS, type LookId } from "./render/looks";
import { createRenderer, type Renderer } from "./render/renderer";
import {
  boxEntry,
  boxFocus,
  boxKey,
  exitSeed,
  pickExitDomain,
  stepBoxDoors,
  steppedAway,
  type DomainRow,
} from "./world/box";
import { atConsoleExit, consoleRoom } from "./world/consoleRoom";
import {
  DIP_MS,
  DIP_SWAP_MS,
  FLICKER_MS,
  darkened,
  diffRooms,
  dipFactor,
  settleSpot,
} from "./world/diff";
import { ACCESS_DENIED, NOT_FOUND, generateRoom } from "./world/generate";
import {
  arrivalSpawn,
  doorKey,
  focusOf,
  hatchTravel,
  approaches,
  stepDoors,
  travelOf,
  wallPoint,
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
import { LIFT_RIDE_MS } from "./timing";
import { LIFT_WORDS, deckLabel } from "./world/lifts";
import { roomFor } from "./world/station";
import type {
  Box,
  Fixture,
  LiftStop,
  PlaceInput,
  RoomSpec,
  StationAddress,
} from "./world/types";

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
 * - `prompt`: the line for what the player faces (`SPACE READ Scope`), null
 *   when nothing.
 * - `status`: the room, its condition and the mouse hint.
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
 * - `initialLook`: the look the session runs in, for good; `aperture`
 *   when not given. Only the dev pages pass another (their `?look=`).
 * - `clock`: the loop's clock; the browser's when not given. Tests crank it
 *   by hand.
 * - `load`: loads a place in place of the query client. The gallery's
 *   `?fault=` answers every travel with a failure through it, and tests use
 *   it to fail a travel without the API. Without it, `go` loads through
 *   `client`, or says `SIGNAL LOST` when there is none.
 * - `onLevels`: the level cheat's switch and channel; only the game route
 *   passes it. See the member.
 * - `consoleRoom`: the police box's inside and the listing its inner doors
 *   pick from; only the game route passes it. See the member.
 * - `onLift`: the lift overlay's channel; only the game route passes it.
 *   See the member.
 * - `onPause`: the pause's channel; only the game route passes it. See
 *   the member.
 * - `sound`: where the cues go (`audio/cues.ts`); only the game route
 *   passes it. Without it the session is silent and M does nothing.
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
  /**
   * The console room's switch and listing (2.6e C11, C13): with it, walking
   * through a police box's open doors cuts into the console room, and
   * walking into the console room's inner doors travels to a domain's
   * bridge picked from `domains`, which answers the domain listing, or
   * null when it could not be read. It is read once per visit of the
   * console room, and its signal aborts once the player leaves the room.
   * Without it a box's doors still open, but walking in does nothing: the
   * look demo and the model gallery never pass it.
   */
  consoleRoom?: {
    domains(signal: AbortSignal): Promise<readonly DomainRow[] | null>;
  };
  /**
   * The lift overlay's channel (M3 C26): with it, Space at a lift calls it
   * with the lift's stops and note and the session gives the overlay the
   * keys; closing the overlay (`closeLift`, `ride`, a `go`, `dispose`)
   * calls it with null. Without it Space at a lift does nothing: the look
   * demo and the model gallery never pass it.
   */
  onLift?: (lift: { stops: LiftStop[]; note: string | null } | null) => void;
  /**
   * The pause's channel (M4 C6): called with true when the station pauses
   * and with false when the pause ends (`resume`, a `go`, `dispose`).
   * Without it the station never pauses: the look demo and the model
   * gallery never pass it.
   */
  onPause?: (paused: boolean) => void;
  /**
   * Where the session sends its sound cues, as it writes text to `hud`
   * (M4 C20 to C23), and whose mute M toggles. See the module doc.
   */
  sound?: SoundSink;
}

/**
 * Loads the station address `address` for a travel or a `go`, and settles
 * as `loadStation` does: what the room is built from, or the reason there
 * is none. It rejects with an `AbortError` once `signal` aborts, and with
 * anything else for a failure the server did not explain (`?LOAD ERROR`).
 * See `SessionOptions.load`.
 */
export type PlaceLoader = (
  address: StationAddress,
  signal: AbortSignal,
) => Promise<LoadedStation>;

/**
 * A running station.
 *
 * - `go` travels to a station address (M3 C1), in its canonical form
 *   (`canonicalStation`: the MANIFEST's engram address is its domain's
 *   bridge, C3): loads it and, once it lands, enters it, placed by
 *   `arrival` (see `arrivalSpawn`) or at the entrance. The connector's
 *   label is `labelFor`'s, unless `label` is given, which shows that
 *   instead (`jump`'s domain name, C10).
 * - `showCanned` shows a place already in hand, with no load. Showing the
 *   place already shown again (the demo's R key) keeps the player where
 *   they stand and the doors as they are.
 * - `showRoom` shows a room built by hand, with no place behind it (the
 *   dev-only model gallery): no load, no navigation, the player at the
 *   room's entrance and every door shut. Its terminals open no reader,
 *   since there is no engram to read. `current` stays null (M3 C5) unless
 *   `at` names the address the room stands for, a seam for the tests that
 *   stand a hand-built room (a police box forced into a deck, say) where
 *   the session would have entered it. `view`, when given, sets the
 *   player's pitch after entering, clamped to `MAX_PITCH` (C18, 2.6b): the
 *   dev seams' close curio framing (`spotView` in `dev/spots.ts`). It is
 *   for those dev seams only; every other caller omits it and keeps the
 *   entrance's own pitch of 0.
 * - `go`, `showCanned` and `showRoom` close an open CRT reader, level
 *   select or lift overlay first.
 * - `closeReader` tells the session the CRT reader was closed, which gives
 *   it the keys back.
 * - `jump` goes to a domain's bridge (`bridgeAddress`), the level select's
 *   jump: a `go` from outside, so the player enters at the entrance. The
 *   connector names the domain (C10).
 * - `closeLevels` tells the session the level select was closed, which
 *   gives it the keys back.
 * - `ride` rides the lift last opened to its stop `stop` (an index into
 *   the stops `onLift` was given): closes the overlay and travels there,
 *   the connector naming the stop, landing no sooner than `LIFT_RIDE_MS`
 *   after the call. An index with no stop does nothing.
 * - `closeLift` tells the session the lift overlay was closed, which gives
 *   it the keys back.
 * - `dispose` stops everything and frees the GPU objects. It takes the
 *   reader and the connector down; nothing is written to the HUD after it.
 * - `current` is the station address the player is in, the one the session
 *   entered (a deck's section resolved), never read back from the room;
 *   null before the first one and in a room `showRoom` showed without an
 *   address. Inside the console room it is the address the player walked
 *   in from, since the console room has no address of its own.
 * - `page` is the address whose Fluid page F and the host's way out open:
 *   `current`, or the address a move sent it to while that move has not
 *   been followed yet (M4 C18), since the old permalink's page is gone.
 * - `where` is the room's label as the status line shows it, the room's
 *   title upper-cased; null before the first room and in the console room,
 *   which has no title.
 * - `pause` pauses the station unless something already has the keys: the
 *   keys are forgotten, the prompt goes, the lock is released and
 *   `onPause` is told. `paused` says whether it is paused.
 * - `resume` ends the pause, tells `onPause` and asks for the lock again,
 *   so it must be called from a user gesture. Not paused, it does nothing.
 * - `setBusy` holds the station modal for a screen of the host's, as an
 *   overlay does, with no pause and no lock released.
 * - `flash` shows the host's notice (`NO CARRIER`, M4 C27) for
 *   `NOTICE_MS`, as a failed load's, then the standing notice or none.
 * - `lockEndedAt` is when the pointer lock last ended, on
 *   `performance.now()`'s clock (`Input.lockEndedAt`): the lock is asked
 *   for again no sooner than `RELOCK_DELAY_MS` after it.
 */
export interface Session {
  go(address: StationAddress, arrival?: Arrival | null, label?: string): void;
  showCanned(place: PlaceInput): void;
  showRoom(room: RoomSpec, view?: { pitch: number }, at?: StationAddress): void;
  closeReader(): void;
  /**
   * Goes to a domain's bridge (`bridgeAddress`): the level select's jump.
   * The connector names the domain (C10).
   */
  jump(domain: string): void;
  /** The level select was closed: gives the session the keys back. */
  closeLevels(): void;
  /** Rides the lift last opened to its stop at index `stop` (M3 C27). */
  ride(stop: number): void;
  /** The lift overlay was closed: gives the session the keys back. */
  closeLift(): void;
  dispose(): void;
  pause(): void;
  resume(): void;
  setBusy(busy: boolean): void;
  flash(text: string): void;
  /**
   * One frame of the change stream, after the host made its keys stale:
   * follows a move of the current engram and re-checks the room when the
   * frame concerns it (M4 C14, C15, C18). See the module doc.
   */
  changed(event: ChangeEvent): void;
  readonly current: StationAddress | null;
  readonly page: StationAddress | null;
  readonly where: string | null;
  readonly paused: boolean;
  readonly lockEndedAt: number;
}

/** Where the inverted-look choice is remembered: "1" inverted, "0" normal. */
export const INVERT_KEY = "station.invertY";

/** How long a failed load's notice stays up, in milliseconds. */
export const NOTICE_MS = 3000;

/** How long the inverted-look notice stays up, in milliseconds. */
export const LOOK_NOTICE_MS = 1500;

/**
 * How long the console room's exit waits for the domain listing, in
 * milliseconds from the cut in: a listing that has not answered by then
 * counts as failed, so the inner doors lead to the room left's own domain
 * rather than nowhere.
 */
export const LISTING_WAIT_MS = 5000;

/** The shortest lift ride (M3 C27), kept in `timing.ts` for the sound. */
export { LIFT_RIDE_MS };

/**
 * How far from its exit's wall point the player must once stand, in
 * metres, before the exit opens (M3 C28): the entrance spawn stands 1 m
 * from it, and a step back would otherwise go straight up again. Not the
 * console room's exit latch (`exitLatched`), which is its own.
 */
export const UP_LATCH_CLEAR = 2.0;

/**
 * How long the pause screen ignores Esc after it opened, in milliseconds
 * (M4 C7): a browser that delivers the Esc that ended the lock must not
 * leave the station on it.
 */
export const PAUSE_ESC_GUARD_MS = 250;

/**
 * How long after one reshape the next may run, in milliseconds (M4 C15):
 * a shape change that comes sooner is held until then.
 */
export const RESHAPE_MIN_MS = 3000;

/** The notice over a power dip (M4 C17). */
export const RECONFIGURING = "STATION RECONFIGURING";

/** No fixture held shut. */
const NONE_SHUT: ReadonlySet<number> = new Set();

/** The notice for a refused or missing GPU. */
const NO_DEVICE = "?DEVICE NOT PRESENT ERROR";

/** The answers a load gives when there is no room to build. */
type Failure = "missing" | "denied" | "offline";

/** What each failed answer tells the player. */
const FAILED: Record<Failure, string> = {
  missing: NOT_FOUND,
  denied: ACCESS_DENIED,
  offline: "SIGNAL LOST",
};

/** The notice for a load that failed in a way the server did not explain. */
const LOAD_ERROR = "?LOAD ERROR";

/**
 * The movement keys by `KeyboardEvent.code`, the classic layout first and
 * WASD beside it. The keys of one direction are ORed, so two held at once
 * move no faster than one. Left and Right turn, and strafe instead while
 * an Alt key is held (matched by code, so macOS Option's special
 * characters do not matter). Up and Down walk; only the mouse looks up
 * and down.
 */
const FORWARD_KEYS = ["ArrowUp", "KeyW"] as const;
const BACK_KEYS = ["ArrowDown", "KeyS"] as const;
const STRAFE_LEFT_KEYS = ["Comma", "KeyA"] as const;
const STRAFE_RIGHT_KEYS = ["Period", "KeyD"] as const;
const ALT_KEYS = ["AltLeft", "AltRight"] as const;
/** Held, the run keys double the walk (`RUN_FACTOR`). */
const RUN_KEYS = ["ShiftLeft", "ShiftRight"] as const;

/**
 * The game's keys whose browser default it cancels while no overlay has
 * the keys and no Ctrl or Cmd is held: Space and the arrows scroll, Alt
 * with Left goes back in the history and a lone Alt opens the menu bar.
 * Letters are left alone.
 */
const CLAIMED_KEYS: ReadonlySet<string> = new Set([
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Space",
  "Comma",
  "Period",
  ...ALT_KEYS,
]);

/** The use key: doors, terminals, the hatch, a police box's doors. */
const USE_KEY = "Space";

/** Every key that commands the session, drained while the reader is open. */
const COMMAND_KEYS = [USE_KEY, "KeyF", "KeyI", "KeyM"] as const;

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

/** Whether a load's answer is one of the three with no room to build. */
function isFailure(
  loaded: LoadedStation,
): loaded is Extract<LoadedStation, { kind: Failure }> {
  return (
    loaded.kind === "missing" ||
    loaded.kind === "denied" ||
    loaded.kind === "offline"
  );
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
  const lookId: LookId = opts.initialLook ?? "aperture";
  let inverted = readInverted();
  let place: PlaceInput | null = null;
  let current: StationAddress | null = null;
  let room: RoomSpec | null = null;
  let blockers: Box[] = [];
  let lights: LightState | null = null;
  /** The blink banks' gains, one state for the whole session (H11). */
  const blink = createBlink();
  let doors = new Map<number, DoorState>();
  /**
   * Every police box's doors, keyed by its index in `room.heroes`
   * (`boxKey`).
   */
  let boxes = new Map<number, DoorState>();
  let doorOpen = new Map<string, number>();
  let player: Player | null = null;
  let previous: Player | null = null;
  let prefetched = new Set<string>();
  let latched: number | null = null;
  let readerOpen = false;
  /** Whether the level select is open (only with `onLevels`). */
  let levelsOpen = false;
  /** Whether the lift overlay is open (only with `onLift`). */
  let liftOpen = false;
  /**
   * The lift whose overlay was last opened in this room, by fixture index;
   * `ride` rides it. Null until one is opened, and on every entry.
   */
  let liftAt: number | null = null;
  /**
   * The ride in flight: when it started and, once its load has settled
   * early, the landing held until `LIFT_RIDE_MS` after that. `leave`
   * drops it.
   */
  let ride: { start: number; held: (() => void) | null } | null = null;
  /**
   * The room's exit while it is latched (M3 C28), by fixture index: set
   * on every entry, cleared once the player stands `UP_LATCH_CLEAR` from
   * its wall point.
   */
  let upLatched: number | null = null;
  /** The level cheat's word, read only when the host passed `onLevels`. */
  const cheat = opts.onLevels === undefined ? null : createCheatReader();
  /** Ticks run so far: the clock the word's gap is counted on (C2). */
  let ticks = 0;
  /** Whether the timed notice up is the inverted-look one (C6). */
  let lookFlash = false;
  /** Whether the timed notice up is a dip's `STATION RECONFIGURING`. */
  let dipFlash = false;
  /** Whether the station is paused (M4 C6). */
  let paused = false;
  /** The footsteps sent so far: the `n` of the next `step` cue. */
  let steps = 0;
  /** Whether a lift ride sent its `depart` and has not sent its `arrive`. */
  let riding = false;
  /** Whether a screen of the host's holds the station (`setBusy`). */
  let busy = false;
  /**
   * Whether something else has the keys: an overlay (the CRT reader, the
   * level select or the lift's stops), the pause or a busy host.
   */
  const modal = () => readerOpen || levelsOpen || liftOpen || paused || busy;
  let lastPrompt: string | null = null;
  /** Ways whose travel failed this visit: fixture index to its seal label. */
  let failed = new Map<number, string>();
  /** The fault clock of every broken way this visit. */
  let faults = new Map<number, Fault>();
  /** The running faults' frames, as the renderer draws them. */
  let faultNow: ReadonlyMap<number, FaultFrame> = new Map();
  /** The travel in flight: its generation and the fixture it went through. */
  let travelling: { gen: number; fixture: number } | null = null;
  /**
   * The visit of the console room the player is in: the room walked in
   * from. Null outside it; every entry clears it and the cut in sets it.
   */
  let inside: { from: StationAddress } | null = null;
  /**
   * The listing the console room's exit picks from: undefined while it is
   * still read, null when it could not be read.
   */
  let exitRows: readonly DomainRow[] | null | undefined = undefined;
  /** Whether the exit fired and the player has not left its doorway since. */
  let exitLatched = false;
  /**
   * The police box walked into (or stepped out of) and not stepped away
   * from since (`steppedAway`).
   */
  let boxLatched: number | null = null;
  /**
   * The answering console in focus and since when (the session's clock),
   * null while none is (M4 C25).
   */
  let answering: { index: number; since: number } | null = null;
  /** The consoles that answered this visit, by fixture index. */
  let answered = new Set<number>();
  /** Aborts the listing read for the console room's visit. */
  let listing: AbortController | null = null;
  /** Gives up on the listing after `LISTING_WAIT_MS`; null when not waiting. */
  let listingTimer: ReturnType<typeof setTimeout> | null = null;
  /** Counts the cuts in, so a listing that answers late is dropped. */
  let visit = 0;

  // Live changes (M4 C14 to C19): see the module doc.
  /** A re-check is owed: asked for while one could not run. */
  let pendingCheck = false;
  /** Whether a re-check's load is in flight. */
  let checking = false;
  /** Aborts the re-check in flight; `leave` and `dispose` abort it. */
  let checkAbort: AbortController | null = null;
  /** The coalescing window a concerning frame opened; null when none. */
  let checkTimer: ReturnType<typeof setTimeout> | null = null;
  /** Waits out `RESHAPE_MIN_MS` for a held shape change; null when none. */
  let reshapeTimer: ReturnType<typeof setTimeout> | null = null;
  /** When the last reshape entered its room, on the session's clock. */
  let lastReshape = -Infinity;
  /**
   * The flicker or the dip running, from `at` on the station's clock
   * (`stationTime`); `next` is the dip's swap, run at `DIP_SWAP_MS`.
   */
  let dip: {
    kind: "flicker" | "dip";
    at: number;
    next: (() => void) | null;
  } | null = null;
  /** The light levels scaled by the dip, reused from frame to frame. */
  let dipLevels = new Float32Array(0);
  /** Whether the room shown is the darkened one (C19). */
  let dark = false;
  /** Where a `moved` frame sent the current engram, until it is entered. */
  let target: StationAddress | null = null;
  /**
   * The arrival and the landing the room was entered with (F35): a
   * re-check builds with both, so it builds the room it is comparing
   * with (the airlock's stop the player came from, a bridge's arrival
   * box).
   */
  let entryArrival: Arrival | null = null;
  let entryLanding: "box" | null = null;

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
  /**
   * When the shader's clock started, moved on by every pause's span, so
   * its effects hold still while paused and go on without a jump.
   */
  let started = now();
  /** When the pause began, on the session's clock; null while running. */
  let pausedAt: number | null = null;
  /**
   * The station's own clock: the session's with every pause cut out, the
   * clock the shader and a dip run on, so both hold still while paused.
   */
  const stationTime = () => now() - started;
  /** The last `renderer.setRoom` call's time, in ms; null before the first. */
  let lastBuildMs: number | null = null;

  // Made before `onClaimedKey` is added: the input drops a keydown that is
  // already cancelled (`input.ts`), and the claimed keys are the game's own.
  const input = createInput(canvas);
  const onClick = () => {
    if (!modal()) input.requestLock();
  };
  canvas.addEventListener("click", onClick);
  const onClaimedKey = (e: KeyboardEvent) => {
    if (modal() || e.ctrlKey || e.metaKey || !CLAIMED_KEYS.has(e.code)) return;
    e.preventDefault();
  };
  window.addEventListener("keydown", onClaimedKey);
  window.addEventListener("keyup", onClaimedKey);
  // The pause's own listeners (M4 C6, C9), after the input's, so the
  // input has stamped the lock's end before a pause is shown. `pause`
  // itself does nothing once disposed or while anything is modal.
  const onPauseLock = () => {
    if (document.pointerLockElement !== canvas) pause();
  };
  const onPauseKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || e.repeat || e.defaultPrevented) return;
    if (document.pointerLockElement !== canvas) pause();
  };
  // A hidden tab stops the loop while the clock runs on: the answering
  // console's wait starts over, as after the pause (M4 C25).
  const onVisibility = () => {
    if (document.visibilityState === "hidden") answering = null;
  };
  document.addEventListener("visibilitychange", onVisibility);
  // Only a host that shows the pause (`onPause`) gets one: the look demo
  // and the model gallery never pass it, and a pause nobody can end would
  // freeze them.
  if (opts.onPause !== undefined) {
    document.addEventListener("pointerlockchange", onPauseLock);
    window.addEventListener("keydown", onPauseKey);
  }

  // The notice shown when no timed one is up: a missing GPU wins over a
  // place that could not be entered.
  const standingNotice = () => deviceNotice ?? placeNotice;

  const showStanding = () => {
    if (disposed || noticeTimer !== null) return;
    hud.notice(standingNotice());
  };

  /** Sends a cue to the sound sink, if any; nothing once disposed. */
  const cue = (c: Cue) => {
    if (!disposed) opts.sound?.cue(c);
  };

  /**
   * Ends a lift ride's sound: its `arrive`, once, whenever the ride ends
   * (its landing, a failed stop, a load error, a new place), so the held
   * ride hum never outlasts the ride.
   */
  const arrive = () => {
    if (!riding) return;
    riding = false;
    cue({ kind: "ride", phase: "arrive" });
  };

  const flash = (text: string, ms: number) => {
    lookFlash = false;
    dipFlash = false;
    if (disposed) return;
    if (noticeTimer !== null) clearTimeout(noticeTimer);
    hud.notice(text);
    noticeTimer = setTimeout(() => {
      noticeTimer = null;
      lookFlash = false;
      dipFlash = false;
      showStanding();
    }, ms);
  };

  /**
   * The room's label as the status line leads with it (`Session.where`):
   * its title upper-cased, or null with no room or in the console room,
   * which has no title.
   */
  const where = (): string | null =>
    room === null || room.title === "" ? null : room.title.toUpperCase();

  const showStatus = () => {
    if (disposed) return;
    const parts: string[] = [];
    if (room !== null) {
      const label = where();
      if (label !== null) parts.push(label);
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
   * The label the connector shows for a station address: a bridge says its
   * domain, a deck its `deckLabel`, the airlock `AIRLOCK`. An engram says
   * its title when the place the player is in knows it (a relation, a
   * wikilink or an inbound reference that leads there), else the label of
   * a door in the room that leads there (a deck's doors carry the
   * engram's title, and a deck has no place to ask), else its permalink.
   * `go`'s own `label` argument wins over this (the level select's jump,
   * C10).
   */
  const labelFor = (address: StationAddress): string => {
    switch (address.kind) {
      case "airlock":
        return LIFT_WORDS.airlock;
      case "bridge":
        return address.domain;
      case "deck":
        return deckLabel(address.domain, address.folder);
      case "engram":
        break;
    }
    if (place !== null) {
      for (const r of [...place.relations, ...place.links]) {
        if (
          r.targetTitle !== null &&
          r.address !== null &&
          sameStation(stationOfPlace(r.address), address)
        ) {
          return r.targetTitle;
        }
      }
      for (const h of place.inbound) {
        if (sameStation(stationOfPlace(h.address), address)) return h.title;
      }
    }
    for (const f of room?.fixtures ?? []) {
      if (
        f.kind === "door" &&
        f.address !== null &&
        sameStation(stationOfPlace(f.address), address)
      ) {
        return f.label;
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
   * generated from, or null for a room with none (the airlock, a deck, the
   * console room, a room built by hand), whose terminals then open no
   * reader. `address` becomes `current`: the station address entered, as
   * `roomFor` resolved it, never read back from `built` (M3 C5). `keep`
   * keeps the player, the doors and
   * this visit's failed ways and faults, for the same place shown again.
   * `spawn`, when given, places the player there (in metres) in place of
   * `arrivalSpawn` (the arrival box's step out). Every entry ends a visit
   * of the console room: `inside` is cleared and its listing read aborted
   * (the cut in sets both again once it has entered). `landing` is the
   * landing the room was entered with (`land`'s), kept with `arrival` for
   * the re-check (F35); every entry leaves the dark room behind unless
   * `darkNow` enters the darkened one (C19). Every entry sends a `room`
   * cue with the room's ambience.
   *
   * The renderer is asked first. When it refuses the room, nothing of the
   * session has changed yet: the player stays in the room they were in,
   * with its blockers, lights and doors, and `fail` says `?LOAD ERROR`.
   * Returns whether the room was entered. Every press not yet consumed is
   * dropped on the way in, so a key hit for the room left behind (a Space
   * while this one loaded) does nothing here.
   */
  const enter = (
    next: PlaceInput | null,
    built: RoomSpec,
    address: StationAddress | null,
    arrival: Arrival | null,
    keep: boolean,
    spawn?: { x: number; z: number; yaw: number },
    landing: "box" | null = null,
    darkNow = false,
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
    current = address;
    entryArrival = arrival;
    entryLanding = landing;
    dark = darkNow;
    if (!keep || player === null) {
      const at = spawn ?? arrivalSpawn(room, arrival);
      player = { ...at, vx: 0, vz: 0, pitch: 0, bob: 0 };
      doors = new Map();
      boxes = new Map();
      doorOpen = new Map();
      failed = new Map();
      faults = new Map();
      faultNow = new Map();
    }
    previous = player;
    prefetched = new Set();
    latched = null;
    boxLatched = null;
    exitLatched = false;
    answering = null;
    answered = new Set();
    liftAt = null;
    const exit = room.fixtures.findIndex((f) => f.kind === "exit");
    upLatched = exit < 0 ? null : exit;
    inside = null;
    listing?.abort();
    listing = null;
    if (listingTimer !== null) clearTimeout(listingTimer);
    listingTimer = null;
    placeNotice = null;
    showStanding();
    showStatus();
    cue({ kind: "room", ambience: ambienceOf(room, dark), seed: room.seed });
    return true;
  };

  /**
   * A load of `travel` has settled. A current one lands (`land`) at once,
   * unless it is a lift ride's that settled before `LIFT_RIDE_MS` had
   * passed: then the landing is held, and the tick runs it once the ride
   * has lasted that long (M3 C27).
   */
  const settle = (
    gen: number,
    arrival: Arrival | null,
    loaded: LoadedStation,
    label: string,
    landing: "box" | null,
  ) => {
    if (disposed || gen !== generation) return;
    if (ride !== null && now() < ride.start + LIFT_RIDE_MS) {
      ride.held = () => {
        land(arrival, loaded, label, landing);
        retryCheck();
      };
      return;
    }
    land(arrival, loaded, label, landing);
    // A re-check held for the load runs now, on the room it left the
    // player in.
    retryCheck();
  };

  /**
   * The doors of the first fixture of `kind` in the room start fully open
   * and heading shut: the lift after a ride, the exit after a walk in
   * through a deck's door (M3 C27, C28).
   */
  const startOpen = (kind: Fixture["kind"]) => {
    const index = room?.fixtures.findIndex((f) => f.kind === kind) ?? -1;
    if (index < 0) return;
    doors = new Map(doors).set(index, { open: 1, target: 0 });
    // A ride lands inside a tick that ends there, so the frame drawn next
    // reads the door open from here rather than from `stepDoors`.
    doorOpen = new Map(doorOpen).set(doorKey(index), 1);
  };

  /**
   * A load of `travel` has landed: builds the room (`roomFor`) and enters
   * it at the address `roomFor` resolved, or fails with the reason there
   * is none. With `landing` `"box"` (the console room's exit, which goes
   * to a bridge) the room is entered with the arrival box
   * (`withArrivalBox`), the player stepping out of it, its doors fully
   * open and heading shut, and its walk-in latched; with no spot for the
   * box, the room is entered plain. Once entered, the URL is replaced with
   * the entered address's game route unless the location's pathname and
   * search already spell it (M3 C5), so no address replaces itself.
   */
  const land = (
    arrival: Arrival | null,
    loaded: LoadedStation,
    label: string,
    landing: "box" | null,
  ) => {
    const gen = generation;
    ride = null;
    loading = false;
    controller = null;
    hud.connector(false, label, lookId);
    arrive();
    if (isFailure(loaded)) {
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
    const built = roomFor(loaded, arrival, landing);
    if (
      !enter(
        built.place,
        built.room,
        built.address,
        arrival,
        false,
        built.spawn ?? undefined,
        landing,
      )
    ) {
      return;
    }
    if (built.box !== null) {
      boxes = new Map([[built.box, { open: 1, target: 0 }]]);
      boxLatched = built.box;
      cue({ kind: "box", phase: "landing" });
    }
    if (arrival?.via === "lift") startOpen("lift");
    else if (arrival?.via === "door" && arrival.from.kind === "deck")
      startOpen("exit");
    showAddress(built.address);
  };

  /**
   * Replaces the URL with `address`'s game route unless the location's
   * pathname and search already spell it (M3 C5): after a landing, and
   * after a re-check that followed a move (C18).
   */
  const showAddress = (address: StationAddress) => {
    const path = gameRouteOf(address);
    // `path` is built with `encodeURIComponent` (M3 C2), which leaves a
    // character like `'` unescaped, while a browser's own query
    // serialiser writes it `%27` - `window.location.search` is always
    // that normalised spelling. Reading `path` back through `URL` before
    // comparing puts both sides through the same normalisation, so a
    // folder name that needs it does not replace the URL on every
    // landing.
    const url = new URL(path, window.location.origin);
    const normalized = url.pathname + url.search;
    const { pathname, search } = window.location;
    if (pathname + search !== normalized) opts.navigate(path);
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
   * reader, the level select, the lift overlay and the pause, drops the load in
   * flight (a new generation, the old one aborted) and a ride's held
   * landing with it, drops a re-check in flight and a dip, and takes the
   * connector down if it was up.
   */
  const leave = (): number => {
    closeReader();
    closeLevels();
    closeLift();
    closePause();
    travelling = null;
    ride = null;
    arrive();
    const gen = ++generation;
    controller?.abort();
    controller = null;
    // A new place drops the re-check in flight, the dip with its swap and
    // a move not yet followed; a re-check owed stays owed, for the room
    // the new place lands in.
    checkAbort?.abort();
    checkAbort = null;
    checking = false;
    dip = null;
    target = null;
    // The dip's notice goes with the dip.
    if (dipFlash && noticeTimer !== null) {
      clearTimeout(noticeTimer);
      noticeTimer = null;
      dipFlash = false;
      showStanding();
    }
    if (loading) {
      loading = false;
      hud.connector(false, loadingLabel, lookId);
    }
    return gen;
  };

  /**
   * The loader a journey and a re-check load with: `SessionOptions.load`,
   * else `loadStation` through the client, else none.
   */
  const loaderOf = (): PlaceLoader | null =>
    opts.load ??
    (client === null ? null : (a, signal) => loadStation(client, a, signal));

  /**
   * Loads `address` and enters it once it lands (`settle`): the journey
   * behind `go` and the console room's exit, which lands with `landing`
   * `"box"`. Without `SessionOptions.load` it loads through `loadStation`.
   */
  const travel = (
    address: StationAddress,
    arrival: Arrival | null,
    label: string | undefined,
    landing: "box" | null,
  ) => {
    if (disposed) return;
    const gen = leave();
    const loader = loaderOf();
    if (loader === null) {
      fail(FAILED.offline);
      return;
    }
    const shown = label ?? labelFor(address);
    loading = true;
    loadingLabel = shown;
    hud.connector(true, shown, lookId);
    const abort = new AbortController();
    controller = abort;
    loader(address, abort.signal).then(
      (loaded) => {
        settle(gen, arrival, loaded, shown, landing);
      },
      (error: unknown) => {
        if (disposed || gen !== generation) return;
        loading = false;
        controller = null;
        travelling = null;
        ride = null;
        arrive();
        hud.connector(false, shown, lookId);
        if (!isAbort(error)) fail(LOAD_ERROR);
        retryCheck();
      },
    );
  };

  const go = (
    address: StationAddress,
    arrival: Arrival | null = null,
    label?: string,
  ) => {
    travel(canonicalStation(address), arrival, label, null);
  };

  const showCanned = (next: PlaceInput) => {
    if (disposed) return;
    leave();
    const address: StationAddress = {
      kind: "engram",
      domain: next.domain,
      permalink: next.permalink,
    };
    const same = sameStation(current, address);
    enter(next, generateRoom(next), address, null, same);
  };

  const showRoom = (
    built: RoomSpec,
    view?: { pitch: number },
    at?: StationAddress,
  ) => {
    if (disposed) return;
    leave();
    if (!enter(null, built, at ?? null, null, false) || view === undefined)
      return;
    if (player === null) return;
    const pitch = Math.max(-MAX_PITCH, Math.min(MAX_PITCH, view.pitch));
    player = { ...player, pitch };
    previous = player;
  };

  /**
   * Goes through a way: to the station address its place names (a way to
   * the MANIFEST lands on the bridge, C3), arriving from `current`, or
   * with no arrival when the room has no address (`showRoom`'s gallery).
   */
  const takeTravel = (travel: Travel) => {
    latched = travel.fixture;
    cue({ kind: "travel", via: travel.via });
    if (travel.via === "exit") {
      // Up to the deck (M3 C28). A failed exit is a notice only (C29):
      // without `travelling`, its failure marks no way.
      go(travel.to, current === null ? null : { via: "exit", from: current });
      return;
    }
    go(
      stationOfPlace(travel.address),
      current === null ? null : { via: travel.via, from: current },
    );
    // Only a travel the session took can mark its way failed (M2).
    if (loading) travelling = { gen: generation, fixture: travel.fixture };
  };

  /**
   * Walks into the police box: cuts to the console room at once, under the
   * station address of the room left (C12), which stays `current`, and
   * starts reading the listing its inner doors pick from, giving up on it
   * after `LISTING_WAIT_MS`. The console room carries the domain of that
   * address (`""` for none) and the room left's own key, its `permalink`
   * (a deck's is its folder's slug with a `/`, M3 C10): an address has no
   * permalink of its own for anything but an engram. A box stands in any
   * room the hero pass runs in (an engram room, a deck, a hangar, a
   * bridge), never the airlock, which has no heroes. Does nothing without
   * the console room option or with no room to return to.
   */
  const cutIn = () => {
    const options = opts.consoleRoom;
    const from = current;
    const key = room?.permalink;
    if (options === undefined || from === null || key === undefined) return;
    leave();
    const built = {
      ...consoleRoom(),
      domain: domainOf(from) ?? "",
      permalink: key,
    };
    if (!enter(null, built, from, null, false)) return;
    cue({ kind: "box", phase: "takeoff" });
    inside = { from };
    exitLatched = false;
    exitRows = undefined;
    const mine = ++visit;
    const abort = new AbortController();
    listing = abort;
    options.domains(abort.signal).then(
      (rows) => {
        if (disposed || mine !== visit || inside === null) return;
        exitRows = rows;
      },
      (error: unknown) => {
        if (disposed || mine !== visit || inside === null || isAbort(error))
          return;
        exitRows = null;
      },
    );
    listingTimer = setTimeout(() => {
      listingTimer = null;
      if (disposed || mine !== visit || inside === null) return;
      if (exitRows === undefined) exitRows = null;
    }, LISTING_WAIT_MS);
  };

  /**
   * Walks out of the console room's inner doors: travels to the bridge of
   * a domain picked from the listing (C13), the connector naming it, to
   * land with the arrival box (C14). The domain of `from`, the address
   * walked in from, is the one the pick leaves out, and the one it falls
   * back to.
   */
  const walkOut = (from: StationAddress) => {
    const own = domainOf(from) ?? "";
    const picked = pickExitDomain(exitRows ?? [], own, exitSeed(own, ticks));
    travel(bridgeAddress(picked), null, picked, "box");
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
    cue({ kind: "terminal" });
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
    cue({ kind: "terminal" });
    opts.onLevels(true);
  };

  /**
   * Closes the level select, for the host's Esc or a new place (`leave`):
   * forgets the half-typed word and every key the overlay took (C4), gives
   * the session the keys back as `closeReader` does, and tells the host.
   * Does nothing when the select is not open.
   */
  const closeLevels = () => {
    if (disposed || !levelsOpen) return;
    levelsOpen = false;
    cheat?.reset();
    input.clear();
    opts.onLevels?.(false);
  };

  /**
   * Space at lift `index` (M3 C26, C29): with `onLift`, outside the
   * console room, opens its stops and gives the overlay the keys and the
   * mouse as the level select does; a lift that reads
   * `?DOMAIN LIST ERROR` reads the airlock again instead.
   */
  const openLift = (index: number) => {
    const lift = room?.fixtures[index];
    if (lift?.kind !== "lift" || opts.onLift === undefined || inside !== null)
      return;
    if (lift.note === LIFT_WORDS.domainError) {
      go({ kind: "airlock" });
      return;
    }
    liftOpen = true;
    liftAt = index;
    cheat?.reset();
    input.clear();
    if (document.pointerLockElement !== null) {
      document.exitPointerLock?.();
    }
    setPrompt(null);
    cue({ kind: "terminal" });
    opts.onLift({ stops: lift.stops, note: lift.note });
  };

  /**
   * Closes the lift overlay, for the host's Esc, a ride or a new place
   * (`leave`): gives the session the keys back and tells the host. Does
   * nothing when the overlay is not open.
   */
  const closeLift = () => {
    if (disposed || !liftOpen) return;
    liftOpen = false;
    cheat?.reset();
    input.clear();
    opts.onLift?.(null);
  };

  /**
   * Pauses the station (M4 C6) unless something already has the keys:
   * forgets every key and the half-typed word, takes the prompt down,
   * releases the lock (the loss that follows finds the session modal) and
   * tells the host.
   */
  const pause = () => {
    if (disposed || modal()) return;
    paused = true;
    pausedAt = now();
    input.clear();
    cheat?.reset();
    setPrompt(null);
    // The tick returns before the focus while paused: the answering
    // console's wait starts over after it (M4 C25).
    answering = null;
    if (document.pointerLockElement !== null) {
      document.exitPointerLock?.();
    }
    opts.onPause?.(true);
  };

  /**
   * Ends the pause without asking for the lock, for `resume` and a new
   * place (`leave`): moves the shader's clock on by the paused span,
   * forgets the keys pressed on the pause screen and tells the host. Returns whether a pause was ended.
   */
  const closePause = (): boolean => {
    if (disposed || !paused) return false;
    paused = false;
    if (pausedAt !== null) started += now() - pausedAt;
    pausedAt = null;
    input.clear();
    cheat?.reset();
    opts.onPause?.(false);
    return true;
  };

  /**
   * Rides the lift last opened to its stop `index` (M3 C27): closes the
   * overlay and travels there, the connector naming the stop. The lift's
   * doors are already heading shut (`stepDoors` never opens them). The
   * ride starts now on the session's clock; its landing waits for
   * `LIFT_RIDE_MS` (`settle`). A failed stop is a notice only (C29): no
   * `travelling`, so no way is marked. Picking the stop the lift already
   * stands at (`stop.here`) just closes the overlay: no ride, no reload of
   * the room the player is already standing in.
   */
  const rideLift = (index: number) => {
    if (disposed) return;
    const lift = liftAt === null ? undefined : room?.fixtures[liftAt];
    const stop = lift?.kind === "lift" ? lift.stops[index] : undefined;
    if (stop === undefined) return;
    closeLift();
    if (stop.here) return;
    const start = now();
    go(
      stop.to,
      current === null ? null : { via: "lift", from: current },
      stop.label,
    );
    if (loading) {
      ride = { start, held: null };
      cue({ kind: "travel", via: "lift" });
      cue({ kind: "ride", phase: "depart" });
      riding = true;
    }
  };

  /**
   * Whether a re-check must wait (M4 C15): a load or a lift ride in
   * flight, an overlay, the pause or a busy host (`modal`), or a flicker
   * or a dip still running.
   */
  const checkBlocked = () =>
    loading || ride !== null || modal() || dip !== null;

  /**
   * Asks for a re-check: runs it now when nothing holds it, else owes it
   * (`pendingCheck`, the latest wins) to whatever holds it.
   */
  const requestCheck = () => {
    if (disposed) return;
    if (checking || reshapeTimer !== null || checkBlocked()) {
      pendingCheck = true;
      return;
    }
    pendingCheck = false;
    recheck();
  };

  /** Runs the re-check owed, if any, once what held it has ended. */
  const retryCheck = () => {
    if (pendingCheck) requestCheck();
  };

  /** The latches an entry resets, for the keep paths to put back. */
  const latches = () => ({ latched, boxLatched, exitLatched, upLatched });
  const restoreLatches = (saved: ReturnType<typeof latches>) => {
    ({ latched, boxLatched, exitLatched, upLatched } = saved);
  };

  /**
   * Re-enters the room shown, rebuilt, keeping the player, the doors, the
   * malfunctions and the latches (a text change, the dark room): the
   * player may stand in an open doorway or a box's, and must not be
   * carried through it by the entry.
   */
  const reenterKept = (
    next: PlaceInput | null,
    built: RoomSpec,
    address: StationAddress | null,
    darkNow = false,
  ): boolean => {
    const saved = latches();
    if (
      !enter(
        next,
        built,
        address,
        entryArrival,
        true,
        undefined,
        entryLanding,
        darkNow,
      )
    )
      return false;
    restoreLatches(saved);
    return true;
  };

  /**
   * The re-check (M4 C15): loads the current address again, or the one a
   * move sent it to, and settles the answer (`applyCheck`) unless a new
   * place was taken meanwhile. One runs at a time; a re-check owed runs
   * after it.
   */
  const recheck = () => {
    const address = target ?? current;
    const loader = loaderOf();
    if (address === null || room === null || inside !== null || loader === null)
      return;
    const gen = generation;
    const abort = new AbortController();
    checkAbort = abort;
    checking = true;
    const settled = (): boolean => {
      if (disposed || gen !== generation || checkAbort !== abort) return false;
      checkAbort = null;
      checking = false;
      return true;
    };
    loader(address, abort.signal).then(
      (loaded) => {
        if (!settled()) return;
        applyCheck(address, loaded);
        retryCheck();
      },
      () => {
        // A failure the server did not explain is not worth a notice for a
        // room the player did not ask for; the next frame tries again.
        if (settled()) retryCheck();
      },
    );
  };

  /**
   * Settles a re-check's answer (M4 C15 to C19). While something holds
   * the station (the pause, an overlay) nothing changes under it: the
   * answer is dropped and the re-check owed, to run again once that ends.
   */
  const applyCheck = (address: StationAddress, loaded: LoadedStation) => {
    if (room === null || inside !== null) return;
    if (checkBlocked()) {
      pendingCheck = true;
      return;
    }
    if (isFailure(loaded)) {
      if (loaded.kind === "offline") {
        flash(FAILED.offline, NOTICE_MS);
        return;
      }
      // Gone or refused: the room goes dark in place, once (C19). The
      // airlock is never darkened; its load degrades instead.
      if (dark || address.kind === "airlock") return;
      const domain = domainOf(address) ?? room.domain;
      if (!reenterKept(place, darkened(room, domain), current, true)) return;
      // The dark room's props are dressed again round its hatch, so the
      // spot the player kept may now be inside one: they step to the
      // nearest free floor, their view kept (as a reshape's swap does).
      if (player !== null && room !== null) {
        const spot = settleSpot(room, player.x, player.z);
        if (spot.x !== player.x || spot.z !== player.z) {
          player = { ...player, x: spot.x, z: spot.z, vx: 0, vz: 0 };
          previous = player;
        }
      }
      dark = true;
      placeNotice = FAILED[loaded.kind];
      showStanding();
      return;
    }
    const built = roomFor(loaded, entryArrival, entryLanding);
    switch (diffRooms(room, built.room)) {
      case "same":
        return;
      case "text":
        if (!reenterKept(built.place, built.room, built.address)) return;
        dip = { kind: "flicker", at: stationTime(), next: null };
        cue({ kind: "terminal" });
        followMove(built.address);
        return;
      case "shape":
        break;
    }
    const wait = lastReshape + RESHAPE_MIN_MS - now();
    if (wait > 0) {
      pendingCheck = true;
      reshapeTimer ??= setTimeout(() => {
        reshapeTimer = null;
        retryCheck();
      }, wait);
      return;
    }
    flash(RECONFIGURING, DIP_MS);
    dipFlash = true;
    dip = {
      kind: "dip",
      at: stationTime(),
      next: () => {
        swapIn(built);
      },
    };
  };

  /**
   * The dip's swap (C17): enters the rebuilt room with the player where
   * they stand, or on the nearest free floor (`settleSpot`), their view
   * kept; the doors start shut and the malfunctions are cleared, since
   * the fixtures' indices changed. A lift overlay opened during the dip
   * closes: its stops belong to the room replaced, as with a `go`.
   */
  const swapIn = (built: ReturnType<typeof roomFor>) => {
    const was = player;
    if (was === null) return;
    closeLift();
    const spot = settleSpot(built.room, was.x, was.z);
    if (
      !enter(
        built.place,
        built.room,
        built.address,
        entryArrival,
        false,
        { x: spot.x, z: spot.z, yaw: was.yaw },
        entryLanding,
      )
    )
      return;
    if (player !== null) {
      player = { ...player, pitch: was.pitch };
      previous = player;
    }
    lastReshape = now();
    followMove(built.address);
  };

  /**
   * After a re-check entered a room: a move followed (C18) is done, and
   * the URL names the new address.
   */
  const followMove = (address: StationAddress) => {
    if (target === null) return;
    target = null;
    showAddress(address);
  };

  /** See `Session.changed`. */
  const changed = (event: ChangeEvent) => {
    if (disposed) return;
    const moved = inside === null ? movedTo(event, target ?? current) : null;
    if (moved !== null) target = moved;
    if (!concerns(event, watchOf(current, room, inside !== null))) return;
    // A window, not a debounce: frames that come inside it join it.
    checkTimer ??= setTimeout(() => {
      checkTimer = null;
      requestCheck();
    }, COALESCE_MS);
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

  const anyHeld = (codes: readonly string[]) =>
    codes.some((code) => input.held(code));
  const axis = (plus: boolean, minus: boolean) =>
    (plus ? 1 : 0) - (minus ? 1 : 0);

  /**
   * Where every door is heading, keyed as `doorOpen` keys the fractions:
   * what `doorCues` compares from tick to tick.
   */
  const doorTargets = (): Map<string, number> => {
    const out = new Map<string, number>();
    for (const [index, state] of doors) out.set(doorKey(index), state.target);
    for (const [index, state] of boxes) out.set(boxKey(index), state.target);
    return out;
  };

  /**
   * The answering console (M4 C25): the terminal in `focus` that `answers`
   * and has not answered this visit is timed, the wait starting over
   * whenever the focus leaves it; after `ANSWER_WAIT_MS` it sends `answer`,
   * placed at its wall point, once.
   */
  const listenForAnswer = (focus: ReturnType<typeof focusOf>) => {
    const fixture =
      room === null || focus === null ? undefined : room.fixtures[focus.index];
    if (
      focus === null ||
      fixture === undefined ||
      player === null ||
      !answers(fixture) ||
      answered.has(focus.index)
    ) {
      answering = null;
      return;
    }
    const t = now();
    if (answering?.index !== focus.index) {
      answering = { index: focus.index, since: t };
      return;
    }
    if (t - answering.since < ANSWER_WAIT_MS) return;
    answered.add(focus.index);
    answering = null;
    const w = wallPoint(fixture.slot);
    cue({ kind: "answer", ...placeCue(player, w.x, w.z) });
  };

  const tick = () => {
    ticks++;
    // A ride whose load settled early lands once it has lasted
    // `LIFT_RIDE_MS` (M3 C27); the new room starts on the next tick.
    const held = ride?.held ?? null;
    if (held !== null && ride !== null && now() >= ride.start + LIFT_RIDE_MS) {
      ride = null;
      held();
      return;
    }
    // The keys typed since the last tick, in order: the level cheat's
    // word is read from them, and they are dropped unread while an
    // overlay has the keys (C4).
    const typed = input.typed();
    if (modal()) {
      // The reader and the level select read their keys themselves;
      // none of them is ours.
      for (const code of COMMAND_KEYS) input.pressed(code);
      input.takeLook();
      // Paused, nothing else runs: no movement, doors, faults, travel,
      // lights or blink (M4 C6).
      if (paused) return;
    } else {
      let matched = false;
      if (cheat !== null) {
        for (const code of typed) {
          if (cheat.feed(code, ticks)) matched = true;
        }
      }
      // Nothing takes a use while a place loads. Its press is dropped here
      // rather than kept for the room that loads or, when the load fails,
      // for this one.
      if (loading) input.pressed(USE_KEY);
      if (input.pressed("KeyI")) {
        inverted = !inverted;
        writeInverted(inverted);
        flash(inverted ? "LOOK INVERTED" : "LOOK NORMAL", LOOK_NOTICE_MS);
        lookFlash = true;
      }
      if (input.pressed("KeyM")) {
        const muted = opts.sound?.toggleMute();
        if (muted !== undefined)
          flash(muted ? "SOUND OFF" : "SOUND ON", LOOK_NOTICE_MS);
      }
      const page = target ?? current;
      if (input.pressed("KeyF") && page !== null) {
        opts.openFluid(fluidRouteOfStation(page));
      }
      if (matched) openLevels();
    }
    // The dip's swap at `DIP_SWAP_MS`, and the end of a flicker or a dip,
    // on the station's clock (C17); a re-check held for it runs then.
    if (dip !== null) {
      const elapsed = stationTime() - dip.at;
      const swap = dip.next;
      if (swap !== null && elapsed >= DIP_SWAP_MS) {
        dip.next = null;
        swap();
        // The rest of this tick read the room left: the new one starts on
        // the next.
        return;
      }
      if (elapsed >= (dip.kind === "dip" ? DIP_MS : FLICKER_MS)) {
        dip = null;
        retryCheck();
      }
    }
    if (room === null || player === null) return;

    const look = modal() || loading ? { dx: 0, dy: 0 } : input.takeLook();
    const still = modal() || loading;
    const alt = anyHeld(ALT_KEYS);
    const left = input.held("ArrowLeft");
    const right = input.held("ArrowRight");
    const run = !still && anyHeld(RUN_KEYS);
    previous = player;
    player = stepPlayer(
      player,
      {
        forward: still ? 0 : axis(anyHeld(FORWARD_KEYS), anyHeld(BACK_KEYS)),
        strafe: still
          ? 0
          : axis(
              anyHeld(STRAFE_RIGHT_KEYS) || (alt && right),
              anyHeld(STRAFE_LEFT_KEYS) || (alt && left),
            ),
        turn: still || alt ? 0 : axis(left, right),
        lookDx: look.dx,
        lookDy: lookDelta(look.dy, inverted),
        run,
      },
      room,
      blockers,
    );
    if (!still) {
      for (let k = stepsBetween(previous.bob, player.bob); k > 0; k--) {
        cue({ kind: "step", foot: steps % 2 === 0 ? 0 : 1, run, n: steps });
        steps++;
      }
    }

    const focus = modal() ? null : focusOf(room, player, doors, failed);
    const boxAt =
      modal() || focus !== null ? null : boxFocus(room, player, boxes);
    let pressedDoor: number | null = null;
    let pressedWay: number | null = null;
    let pressedBox: number | null = null;
    const used = !still && input.pressed(USE_KEY);
    if (used && focus !== null) {
      if (isBrokenWay(room, focus.index, failed)) pressedWay = focus.index;
      if (focus.kind === "terminal") {
        openReader(focus.index);
      } else if (focus.kind === "door") {
        pressedDoor = focus.index;
      } else if (focus.kind === "hatch") {
        // A failed hatch carries no one: `hatchTravel` reads `failed`.
        const travel = hatchTravel(room, focus.index, failed);
        if (travel !== null) takeTravel(travel);
      } else if (focus.kind === "lift" && !failed.has(focus.index)) {
        openLift(focus.index);
      }
    } else if (used && boxAt !== null) {
      pressedBox = boxAt.index;
    }
    // The exit's latch holds until the player has once stood
    // `UP_LATCH_CLEAR` from its wall point (M3 C28).
    if (upLatched !== null) {
      const exit = room.fixtures[upLatched];
      if (exit === undefined) {
        upLatched = null;
      } else {
        const w = wallPoint(exit.slot);
        if (Math.hypot(player.x - w.x, player.z - w.z) > UP_LATCH_CLEAR)
          upLatched = null;
      }
    }
    const shut = upLatched === null ? NONE_SHUT : new Set([upLatched]);
    const targetsBefore = doorTargets();
    const faultsBefore = faults;
    doors = stepDoors(room, player, doors, pressedDoor, failed, shut);
    boxes = stepBoxDoors(room, boxes, pressedBox);
    for (const c of doorCues(room, player, targetsBefore, doorTargets()))
      cue(c);
    faults = stepFaults(room, player, faults, failed, pressedWay, doors);
    for (const c of faultCues(room, player, faultsBefore, faults)) cue(c);
    faultNow = faultFrames(faults);
    doorOpen = new Map();
    for (const [index, state] of doors)
      doorOpen.set(doorKey(index), state.open);
    for (const [index, state] of boxes) doorOpen.set(boxKey(index), state.open);
    // The walk-in latch holds until the player has stepped
    // `BOX_LATCH_CLEAR` away from the latched box's front, whatever its
    // doors do meanwhile, so a player who steps out of the arrival box
    // still walking, or turns and opens it again on the spot, walks in
    // only after a real step away and back (C11, C14, C29).
    if (boxLatched !== null) {
      const latched = room.heroes[boxLatched];
      if (latched === undefined || steppedAway(latched, player))
        boxLatched = null;
    }
    if (opts.consoleRoom !== undefined && !loading && !modal()) {
      const entered = boxEntry(room, player, boxes);
      if (entered !== null && entered !== boxLatched) {
        // Latched before the cut: a console room the renderer refuses
        // leaves the player outside with one notice, not one a tick.
        boxLatched = entered;
        cutIn();
        // The rest of this tick read the room left: the console room
        // starts on the next.
        return;
      }
    }
    if (inside !== null && !loading && !modal()) {
      if (!atConsoleExit(player)) {
        exitLatched = false;
      } else if (!exitLatched && exitRows !== undefined) {
        exitLatched = true;
        walkOut(inside.from);
        return;
      }
    }
    setPrompt(
      modal() || loading ? null : (focus?.prompt ?? boxAt?.prompt ?? null),
    );
    listenForAnswer(loading ? null : focus);

    if (!loading && !modal()) {
      const travel = travelOf(room, player, doors, failed, shut);
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

  /**
   * The light levels to draw: `levels` as they are, or scaled by the
   * flicker's or the dip's factor into one array reused frame to frame.
   */
  const levelsNow = (levels: Float32Array): Float32Array => {
    if (dip === null) return levels;
    const factor = dipFactor(dip.kind, stationTime() - dip.at);
    if (dipLevels.length !== levels.length)
      dipLevels = new Float32Array(levels.length);
    for (let i = 0; i < levels.length; i++) dipLevels[i] = levels[i]! * factor;
    return dipLevels;
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
        // Paused, the screen covers the station: nothing is drawn, and the
        // last frame stays as it was (M4 C6).
        if (paused) return;
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
          levelsNow(lights.levels),
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

  // The host's closers run a re-check held for the overlay; `leave` calls
  // the inner ones, before it drops the load, so it never starts one.
  return {
    go,
    showCanned,
    showRoom,
    closeReader() {
      closeReader();
      retryCheck();
    },
    jump(domain) {
      cue({ kind: "jump" });
      go(bridgeAddress(domain), null, domain);
    },
    closeLevels() {
      closeLevels();
      retryCheck();
    },
    ride: rideLift,
    closeLift() {
      closeLift();
      retryCheck();
    },
    pause,
    resume() {
      if (!closePause()) return;
      input.requestLock();
      retryCheck();
    },
    setBusy(next) {
      if (disposed || busy === next) return;
      busy = next;
      // A key the host's screen took is not replayed as a command.
      input.clear();
      cheat?.reset();
      if (!next) retryCheck();
    },
    flash(text) {
      flash(text, NOTICE_MS);
    },
    changed,
    dispose() {
      if (disposed) return;
      // The host's overlays go down with the session, so a host that
      // outlives it (StrictMode's second mount) starts clean.
      hud.reader(null);
      opts.onLevels?.(false);
      opts.onLift?.(null);
      opts.onPause?.(false);
      hud.connector(false, loadingLabel, lookId);
      disposed = true;
      generation++;
      controller?.abort();
      controller = null;
      listing?.abort();
      listing = null;
      if (listingTimer !== null) clearTimeout(listingTimer);
      listingTimer = null;
      if (noticeTimer !== null) clearTimeout(noticeTimer);
      noticeTimer = null;
      checkAbort?.abort();
      checkAbort = null;
      if (checkTimer !== null) clearTimeout(checkTimer);
      checkTimer = null;
      if (reshapeTimer !== null) clearTimeout(reshapeTimer);
      reshapeTimer = null;
      dip = null;
      loop.stop();
      observer?.disconnect();
      ratioQuery?.removeEventListener("change", onRatioChange);
      ratioQuery = null;
      canvas.removeEventListener("click", onClick);
      window.removeEventListener("keydown", onClaimedKey);
      window.removeEventListener("keyup", onClaimedKey);
      // Before `input.dispose()`, which releases the lock: going down
      // pauses nothing (F18).
      document.removeEventListener("pointerlockchange", onPauseLock);
      window.removeEventListener("keydown", onPauseKey);
      document.removeEventListener("visibilitychange", onVisibility);
      canvas.removeEventListener("webglcontextlost", onLost);
      canvas.removeEventListener("webglcontextrestored", onRestored);
      input.dispose();
      renderer?.dispose();
      renderer = null;
    },
    get current() {
      return current;
    },
    get page() {
      return target ?? current;
    },
    get where() {
      return where();
    },
    get paused() {
      return paused;
    },
    get lockEndedAt() {
      return input.lockEndedAt;
    },
  };
}
