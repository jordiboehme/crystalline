/**
 * The look demo's engine room: the session on a canned place.
 *
 * `startDemo` starts a session with no query client, shows `options.place`
 * (the canned bridge unless the caller names another) and returns the one
 * function that undoes all of it. That function is the whole contract with
 * the React shell: StrictMode mounts, unmounts and mounts the shell again
 * in development, which is the only mode this demo exists in, so a cleanup
 * that missed a listener or a frame request would leave two stations
 * running on one canvas.
 *
 * Everything the game does in a room works here: the doors open, the
 * terminals open the CRT reader, the keys are the game's. Only travel goes
 * nowhere, since there is no client to load a place with: walking through a
 * door says `SIGNAL LOST` and leaves the player on the place shown.
 *
 * Keys: 1, 2 and 4 pick the look, R toggles the retired condition (the
 * place's own status; retired shows `archived`, the derelict end of the
 * scale), WASD walks, the arrows turn, the mouse looks once the canvas is
 * clicked, E uses what the player faces and I inverts the vertical look.
 *
 * `options.props` false shows the place undressed: `session.showRoom` with
 * `generateRoom`'s props, heroes and curios stripped, rather than `session.showCanned`, which
 * is the dev-only comparison `?props=0` reads. That path has no client-side
 * `PlaceInput` kept by the session, so its terminals open no reader; R
 * still swaps the condition, rebuilding the same way.
 *
 * `options.hero` forces a hero into the shown room, through the hero pass's
 * own forced-draws path (`roomWithForcedHero`): a canned demo room draws at
 * most one hero of its own (the canned bridge a flying cloud, the hub a
 * hoverboard, the workshop none), so this is
 * how the demo shows any kind standing in a real, generated room rather
 * than only in the hand-built hero hall. The question block is forced
 * through its own draw and the five any-archetype kinds through the draw
 * that fills a free slot, the rest through a pool slot. It overrides
 * `options.props`, since the point is to see the hero dressed into the
 * room, not undressed. A kind that finds no fitting place (its own room's
 * hall has no spot the moat rule and its placement leave clear) falls back
 * to the room drawn without it, never throwing; the HUD's frame line names
 * the hero once it lands (`HERO <KIND>`), and stays quiet otherwise. R
 * rebuilds with the same forced kind, so it still shows whether the hero
 * holds through a condition switch.
 *
 * `options.curio` forces a curio into the shown room the same way, through
 * the curio pass's own forced-draws path (`roomWithForcedCurio`): a canned
 * demo room draws curios of its own too, but rarely the kind being judged,
 * so this is how one is shown for certain. It also overrides
 * `options.props`, like `hero`, and takes priority over it when both are
 * given (only one of `HERO <KIND>` or `CURIO <KIND>` is ever what the demo
 * is being pointed at). A kind that finds no host in this room (its slot's
 * classes hold no surface the room carries) falls back to the room drawn
 * without it, never throwing; the HUD's frame line names it once it lands
 * (`CURIO <KIND>`).
 *
 * `options.at`, read only together with `options.hero` or `options.curio`,
 * runs `spotView` on the room those build and, when it finds a spot, shows
 * the room there with that spot's pitch instead of at its entrance (C18,
 * 2.6b): the same close, tilted framing the gallery's `?at=` reads,
 * without leaving the look demo.
 */

import { createSession, type HudSink, type Session } from "../session";
import { CANNED_BRIDGE } from "../world/canned";
import {
  CURIO_CATALOGUE,
  placeCurios,
  type CurioDraws,
  type CurioSlot,
  type SlotDraw,
} from "../world/curios";
import { dressRoom } from "../world/dress";
import { generateRoom } from "../world/generate";
import {
  ANY_POOL,
  HERO_POOLS,
  heroCap,
  placeHeroes,
  type HeroDraws,
} from "../world/heroes";
import { dressingSites } from "../world/sites";
import type {
  Archetype,
  CurioKind,
  HeroKind,
  PlaceInput,
  RoomSpec,
} from "../world/types";
import { spotView } from "./spots";

/** The status R switches to. */
const RETIRED_STATUS = "archived";

/**
 * The archetype whose pool `kind` is drawn from: `own` when its own pool
 * already holds it, else the first archetype in `HERO_POOLS`'s own order
 * that does. Every pool kind sits in at least one archetype's pool
 * (`heroes.test.ts` pins it), so this only falls back to `own` for the
 * slab, the turret, the block and the any-archetype kinds, which
 * `forcedHeroDraws` forces apart and never asks it for.
 */
function poolArchetypeFor(kind: HeroKind, own: Archetype): Archetype {
  if (HERO_POOLS[own].some(([k]) => k === kind)) return own;
  const archetypes = Object.keys(HERO_POOLS) as Archetype[];
  return archetypes.find((a) => HERO_POOLS[a].some(([k]) => k === kind)) ?? own;
}

/**
 * The forced draws that make `placeHeroes` try `kind` before anything else
 * (exposing forced draws directly as `{slab, turret, picks}`), and the
 * archetype to place it under. The slab, the turret and the question
 * block force their own draw, since `placeHeroes` draws them apart from
 * the archetype's pool; the five any-archetype kinds (`ANY_POOL`) force
 * the any-archetype draw to take with the roll at the middle of their
 * weight, and every pool slot to stay empty, so that draw finds its slot
 * free.
 * Any other kind forces the first pool slot to roll exactly `kind`, the
 * same arithmetic `pickByRoll` runs, out of `poolArchetypeFor`'s pool
 * rather than `room`'s own, so `?hero=` forces a kind foreign to the shown
 * room's own archetype too: it forces that hero into the shown room, not
 * only into a room of its own kind of archetype. `placeHeroes` itself
 * never throws on a kind that finds no fitting candidate: it just leaves
 * the slot empty, so the fallback this function exists for (a hall with no
 * place for the kind) falls out of the draws alone.
 */
function forcedHeroDraws(
  kind: HeroKind,
  room: Pick<RoomSpec, "archetype" | "hall">,
): { draws: HeroDraws; archetype: Archetype } {
  const picks: { take: boolean; roll: number }[] = Array.from(
    { length: heroCap(room.hall) },
    () => ({ take: false, roll: 0 }),
  );
  if (kind === "black-slab") {
    return {
      draws: { slab: true, turret: false, picks },
      archetype: room.archetype,
    };
  }
  if (kind === "turret") {
    return {
      draws: { slab: false, turret: true, picks },
      archetype: room.archetype,
    };
  }
  if (kind === "question-block")
    return {
      draws: { slab: false, turret: false, block: true, picks },
      archetype: room.archetype,
    };
  const anyIndex = ANY_POOL.findIndex(([k]) => k === kind);
  if (anyIndex !== -1) {
    const before = ANY_POOL.slice(0, anyIndex).reduce((s, [, w]) => s + w, 0);
    const total = ANY_POOL.reduce((s, [, w]) => s + w, 0);
    const w = ANY_POOL[anyIndex]?.[1] ?? 0;
    return {
      draws: {
        slab: false,
        turret: false,
        picks,
        any: { take: true, roll: (before + w / 2) / total },
      },
      archetype: room.archetype,
    };
  }
  const archetype = poolArchetypeFor(kind, room.archetype);
  const pool = HERO_POOLS[archetype];
  const index = pool.findIndex(([k]) => k === kind);
  const entry = pool[index];
  if (index === -1 || entry === undefined) {
    return { draws: { slab: false, turret: false, picks }, archetype };
  }
  const before = pool.slice(0, index).reduce((sum, [, w]) => sum + w, 0);
  const total = pool.reduce((sum, [, w]) => sum + w, 0);
  picks[0] = { take: true, roll: (before + entry[1] / 2) / total };
  return { draws: { slab: false, turret: false, picks }, archetype };
}

/**
 * The room `place` becomes with `kind` forced into it: `generateRoom`'s
 * room, its heroes replaced by `placeHeroes` run again on
 * `forcedHeroDraws`'s draws (under the pool archetype the kind forces, not
 * necessarily the room's own: `placeHeroes` reads `room.archetype` only to
 * pick the pool, so this is the one field the forced call overrides) and
 * its props re-dressed (`dressRoom`) to keep off what that hero reserves,
 * exactly as `generateRoom` dresses a room around the heroes it draws on
 * its own. Its curios are placed again (`placeCurios`) on the re-dressed
 * room, with the room's own draws, so no curio stands on a host that moved
 * or was dropped. The returned room keeps its own archetype throughout;
 * only the `placeHeroes` call sees the forced one. `placed` is `kind` when it
 * landed, else null: the caller reads it to decide whether to say so on
 * the HUD.
 */
export function roomWithForcedHero(
  place: PlaceInput,
  kind: HeroKind,
): { room: RoomSpec; placed: HeroKind | null } {
  const built = generateRoom(place);
  const { draws, archetype } = forcedHeroDraws(kind, built);
  const sites = dressingSites(built);
  const heroes = placeHeroes({ ...built, archetype }, draws, sites);
  const placed = heroes.length > 0 ? kind : null;
  const withHeroes: RoomSpec = { ...built, heroes };
  const withProps: RoomSpec = { ...withHeroes, props: dressRoom(withHeroes) };
  return { room: { ...withProps, curios: placeCurios(withProps) }, placed };
}

/**
 * The draws that make `placeCurios` try `kind` before anything else in its
 * own slot (`CURIO_CATALOGUE[kind].slot`): that slot's draw is `{ take:
 * true, roll: 0, kind }` (`roll: 0` always picks the first fitting kind in
 * `pickByRoll`'s order, and a forced draw's pool is only `kind` anyway, so
 * the roll never matters), a floor ball's `floor` is false (it tries the
 * surfaces, not the hall's corners, since a forced kind is meant to be
 * judged sitting on something), the two paired slots' `paired` is false
 * (a forced radar or capsule case lands by its own draw, never by its
 * partner), and the other six slots take nothing.
 */
function forcedCurioDraws(kind: CurioKind): CurioDraws {
  const untaken: SlotDraw = { take: false, roll: 0 };
  const forced: SlotDraw = { take: true, roll: 0, kind };
  const slot = CURIO_CATALOGUE[kind].slot;
  const pick = (mine: CurioSlot): SlotDraw =>
    mine === slot ? forced : untaken;
  return {
    retro: pick("retro"),
    gear: pick("gear"),
    ball: { ...pick("ball"), floor: false },
    under: pick("under"),
    tech: pick("tech"),
    radar: { ...pick("radar"), paired: false },
    capsule: { ...pick("capsule"), paired: false },
  };
}

/**
 * The room `place` becomes with curio `kind` forced into it: `generateRoom`'s
 * room, its curios replaced by `placeCurios` run again on the same room with
 * `forcedCurioDraws(kind)` in place of the room's own draws. Curios never
 * move anything else, so nothing here needs re-dressing the way
 * `roomWithForcedHero` does. `placed` is `kind` when it landed (it found a
 * host among the room's own surfaces), else null: the caller reads it to
 * decide whether to say so on the HUD.
 */
export function roomWithForcedCurio(
  place: PlaceInput,
  kind: CurioKind,
): { room: RoomSpec; placed: CurioKind | null } {
  const built = generateRoom(place);
  const curios = placeCurios(built, forcedCurioDraws(kind));
  const placed = curios.some((c) => c.kind === kind) ? kind : null;
  return { room: { ...built, curios }, placed };
}

/**
 * Starts the look demo on `canvas` and returns its cleanup and the session,
 * which the shell needs to close the CRT reader.
 *
 * `options.forceRgba8` skips the half-float probe, so the RGBA8 bloom path
 * Safari takes can be judged on any browser. `options.openFluid` is where F
 * sends the engram's Fluid page. `options.place` is the canned bridge
 * unless the caller names another, and `options.props` is true unless the
 * caller asks for the undressed comparison (no props and no heroes). R shows the same place again
 * with its status swapped, which keeps the player where they stand.
 *
 * `options.hero` forces that kind into the room instead (`roomWithForcedHero`),
 * which overrides `options.props`: the room is always shown dressed, since
 * the point is the hero standing among real set dressing. Like the
 * undressed path, it goes through `session.showRoom` rather than
 * `showCanned`, so it keeps no client-side `PlaceInput` and its terminals
 * open no reader; R still rebuilds with the same forced kind, resetting the
 * player to the room's entrance the way `showRoom` always does.
 *
 * `options.curio` does the same for a curio kind (`roomWithForcedCurio`),
 * taking priority over `options.hero` when both are given: only one of them
 * is what a given shot is judging. `options.at`, read only alongside
 * `options.hero` or `options.curio`, runs `spotView` on the room `curio` or
 * `hero` built and shows it there instead of at the entrance, with that
 * spot's pitch; ignored otherwise, and ignored (with a spawn at the
 * entrance) when the spot does not resolve.
 */
export function startDemo(
  canvas: HTMLCanvasElement,
  hud: HudSink,
  options: {
    forceRgba8: boolean;
    openFluid: (path: string) => void;
    place?: PlaceInput;
    props?: boolean;
    hero?: HeroKind;
    curio?: CurioKind;
    at?: string;
  },
): { session: Session; stop: () => void } {
  const place = options.place ?? CANNED_BRIDGE;
  const withProps = options.props ?? true;
  const hero = options.hero;
  const curio = options.curio;
  const at = options.at;
  let retired = false;
  let placedHero: HeroKind | null = null;
  let placedCurio: CurioKind | null = null;
  const labelledHud: HudSink =
    hero === undefined && curio === undefined
      ? hud
      : {
          ...hud,
          frame: (text) => {
            const labels = [
              placedHero === null ? null : `HERO ${placedHero.toUpperCase()}`,
              placedCurio === null
                ? null
                : `CURIO ${placedCurio.toUpperCase()}`,
            ].filter((label): label is string => label !== null);
            hud.frame(
              labels.length === 0 ? text : `${text}  ${labels.join("  ")}`,
            );
          },
        };
  const session = createSession({
    canvas,
    client: null,
    hud: labelledHud,
    navigate: () => {},
    openFluid: options.openFluid,
    forceRgba8: options.forceRgba8,
  });
  /** Shows a room built by `hero` or `curio`, framed at `at` when it resolves. */
  const showBuilt = (room: RoomSpec) => {
    const spot = at === undefined ? null : spotView(room, at);
    session.showRoom(
      spot === null ? room : { ...room, spawn: spot.spawn },
      spot === null ? undefined : { pitch: spot.pitch },
    );
  };
  const show = (p: PlaceInput) => {
    if (curio !== undefined) {
      const forced = roomWithForcedCurio(p, curio);
      placedCurio = forced.placed;
      showBuilt(forced.room);
    } else if (hero !== undefined) {
      const forced = roomWithForcedHero(p, hero);
      placedHero = forced.placed;
      showBuilt(forced.room);
    } else if (withProps) session.showCanned(p);
    else
      session.showRoom({
        ...generateRoom(p),
        props: [],
        heroes: [],
        curios: [],
      });
  };
  show(place);

  const onKey = (event: KeyboardEvent) => {
    if (event.code !== "KeyR" || event.repeat) return;
    retired = !retired;
    show({
      ...place,
      status: retired ? RETIRED_STATUS : place.status,
    });
  };
  window.addEventListener("keydown", onKey);

  return {
    session,
    stop: () => {
      window.removeEventListener("keydown", onKey);
      session.dispose();
    },
  };
}
