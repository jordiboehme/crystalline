/**
 * STATION LOOK DEMO: the milestone 1 test room, for picking a look.
 *
 * A full-screen canvas with a small HUD: the look and the room's condition,
 * the frame time, and the keys. Development only - the route that renders
 * this exists only under `import.meta.env.DEV`, so a production build never
 * contains it. `?bloom=rgba8` forces the fallback bloom path Safari takes
 * without float targets, and `?nogl` shows the refusal screen, so both can
 * be checked on any browser.
 *
 * Dev-only query parameters for the browser comparison (ruling 19), none of
 * them shown on the HUD's legend since they are for the controller
 * comparing runs, not a player: `?room=bridge|workshop|hub` picks the place
 * (the canned bridge when absent), `?type=` and `?status=` override that
 * place's own, and `?props=0` shows the same room with its props and heroes
 * stripped, so a build and a frame time can be judged dressed against
 * undressed. The HUD's frame line carries the comparison's other half,
 * `BUILD <ms> MS`: `session.ts` times every `renderer.setRoom` itself.
 *
 * `?hero=<kind>` (the controller's ruling) forces that hero into the shown
 * room, through the hero pass's own forced-draws path
 * (`roomWithForcedHero` in `dev/demo.ts`), so a hero can be judged standing
 * in a real generated room rather than only in the hand-built hero hall the
 * model gallery's `?hall=heroes` shows. A kind that finds no fitting place
 * in this room's archetype or hall falls back to the room drawn without it,
 * never throwing; the HUD's frame line names the hero once it lands
 * (`HERO <KIND>`). An unknown kind is ignored, same as an absent `?hero=`.
 *
 * `?curio=<kind>` (C18, 2.6b) does the same for a curio kind
 * (`roomWithForcedCurio`), so one can be judged standing on real set
 * dressing rather than only in the hero hall's own curios. It takes
 * priority over `?hero=` when both are given, and the HUD names it
 * `CURIO <KIND>` once it lands; an unknown kind is ignored, same as an
 * absent `?curio=`.
 *
 * `?prop=<kind>` (2.6d C20) does the same for a rare prop kind, the poster,
 * the canisters, the tower, the console or the marked crate
 * (`roomWithForcedProp`), so one can be judged in a real room although it
 * lands in few rooms of its own. It ranks below `?curio=` and `?hero=`, and
 * the HUD names it `PROP <KIND>` once it lands; an unknown kind is
 * ignored, same as an absent `?prop=`.
 *
 * `?at=prop:<kind>:<n>[:back|:side|:quarter|:close]`, read only alongside
 * `?hero=`, `?curio=` or `?prop=`, frames the forced thing instead of
 * showing the room from its entrance (`spotView`, the same pattern and
 * views the model gallery's `?at=` reads).
 *
 * The screen is `ui/StationView.tsx`, shared with the game route and the
 * model gallery, and its HUD is `ui/Hud.tsx`, the game's own: its text lines are written
 * straight into the DOM through refs, not through React state, because the
 * frame time changes four times a second and a React render of the shell
 * for each would cost more than the number is worth.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { engramRoute } from "../../paths";
import { detectEnvironment, refusalReason, type Refusal } from "../device";
import { hasWebGL2 } from "../gl/context";
import type { Session } from "../session";
import { DeviceRefusal } from "../ui/DeviceRefusal";
import { CLASSIC_KEYS } from "../ui/keys";
import { StationView } from "../ui/StationView";
import { useHud } from "../ui/useHud";
import { CANNED_BRIDGE, CANNED_HUB, CANNED_WORKSHOP } from "../world/canned";
import { CURIO_KINDS } from "../world/curios";
import { HERO_KINDS } from "../world/heroes";
import { RARE_PROP_KINDS, type RarePropKind } from "../world/props";
import type { CurioKind, HeroKind, PlaceInput } from "../world/types";
import { startDemo } from "./demo";

/** The keys, along the top of the screen. */
const LEGEND = `STATION LOOK DEMO · ${CLASSIC_KEYS} · WASD MOUSE · F FLUID · I INVERT · 1 DAY SHIFT · 2 APERTURE GRID · 4 FREESCAPE 64 · R RETIRED`;

/** The places `?room=` picks between. */
const ROOMS: Record<string, PlaceInput> = {
  bridge: CANNED_BRIDGE,
  workshop: CANNED_WORKSHOP,
  hub: CANNED_HUB,
};

/** Opens a Fluid page in a new tab, as the F key does in the game. */
function openFluid(path: string) {
  window.open(path, "_blank", "noopener");
}

/**
 * The place `?room`, `?type` and `?status` pick: `?room` chooses between
 * `ROOMS` (the canned bridge when absent or unknown), and `?type` and
 * `?status` then override that place's own, for judging a look or a density
 * that a canned place does not happen to carry.
 */
function placeFor(params: URLSearchParams): PlaceInput {
  const room = params.get("room");
  const base = (room !== null ? ROOMS[room] : undefined) ?? CANNED_BRIDGE;
  const type = params.get("type");
  const status = params.get("status");
  return {
    ...base,
    type: type ?? base.type,
    status: status ?? base.status,
  };
}

/** The hero kind `?hero=` names, or undefined for an absent or unknown one. */
function heroFor(params: URLSearchParams): HeroKind | undefined {
  const raw = params.get("hero");
  return raw !== null && (HERO_KINDS as readonly string[]).includes(raw)
    ? (raw as HeroKind)
    : undefined;
}

/** The curio kind `?curio=` names, or undefined for an absent or unknown one. */
function curioFor(params: URLSearchParams): CurioKind | undefined {
  const raw = params.get("curio");
  return raw !== null && (CURIO_KINDS as readonly string[]).includes(raw)
    ? (raw as CurioKind)
    : undefined;
}

/** The rare prop kind `?prop=` names, or undefined for an absent or unknown one. */
function propFor(params: URLSearchParams): RarePropKind | undefined {
  const raw = params.get("prop");
  return raw !== null && (RARE_PROP_KINDS as readonly string[]).includes(raw)
    ? (raw as RarePropKind)
    : undefined;
}

/**
 * The demo screen. The refusal is decided once, in a lazy state
 * initialiser: the lazy route only renders in a browser, where `window` is
 * there, and deciding before the first paint means the canvas never flashes
 * up on a device that is about to be refused. The session starts in an
 * effect once the canvas exists, and the effect's cleanup is `startDemo`'s.
 * A terminal read with Space mounts the CRT reader over the canvas; closing it
 * hands the keys back to the session.
 */
export default function LookDemo() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sessionRef = useRef<Session | null>(null);
  const { sink, view, connector, reader } = useHud();
  const [refusal] = useState<Refusal | null>(() => {
    const params = new URLSearchParams(window.location.search);
    if (params.has("nogl")) return "no-webgl2";
    return refusalReason(detectEnvironment(hasWebGL2));
  });

  useEffect(() => {
    const canvas = canvasRef.current;
    if (refusal !== null || canvas === null) return;
    const params = new URLSearchParams(window.location.search);
    const hero = heroFor(params);
    const curio = curioFor(params);
    const prop = propFor(params);
    const at = params.get("at") ?? undefined;
    const { session, stop } = startDemo(canvas, sink, {
      forceRgba8: params.get("bloom") === "rgba8",
      openFluid,
      place: placeFor(params),
      props: params.get("props") !== "0",
      ...(hero === undefined ? {} : { hero }),
      ...(curio === undefined ? {} : { curio }),
      ...(prop === undefined ? {} : { prop }),
      ...(at === undefined ? {} : { at }),
    });
    sessionRef.current = session;
    return () => {
      sessionRef.current = null;
      stop();
    };
  }, [refusal, sink]);

  const closeReader = useCallback(() => {
    sessionRef.current?.closeReader();
  }, []);
  const readerOpenFluid = useCallback(() => {
    const current = sessionRef.current?.current;
    if (current) openFluid(engramRoute(current.domain, current.permalink));
  }, []);

  if (refusal !== null) return <DeviceRefusal />;
  return (
    <StationView
      canvasRef={canvasRef}
      view={view}
      connector={connector}
      reader={reader}
      legend={LEGEND}
      onCloseReader={closeReader}
      onOpenFluid={readerOpenFluid}
    />
  );
}
