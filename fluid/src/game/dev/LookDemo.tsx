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
 * place's own, and `?props=0` shows the same room with its set dressing
 * stripped, so a build and a frame time can be judged dressed against
 * undressed. The HUD's frame line carries the comparison's other half,
 * `BUILD <ms> MS`: `session.ts` times every `renderer.setRoom` itself.
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
import { StationView } from "../ui/StationView";
import { useHud } from "../ui/useHud";
import { CANNED_BRIDGE, CANNED_HUB, CANNED_WORKSHOP } from "../world/canned";
import type { PlaceInput } from "../world/types";
import { startDemo } from "./demo";

/** The keys, along the top of the screen. */
const LEGEND =
  "STATION LOOK DEMO · 1 DAY SHIFT · 2 APERTURE GRID · 4 FREESCAPE 64 · R RETIRED · WASD ARROWS MOUSE · E USE · F FLUID · I INVERT";

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

/**
 * The demo screen. The refusal is decided once, in a lazy state
 * initialiser: the lazy route only renders in a browser, where `window` is
 * there, and deciding before the first paint means the canvas never flashes
 * up on a device that is about to be refused. The session starts in an
 * effect once the canvas exists, and the effect's cleanup is `startDemo`'s.
 * A terminal read with E mounts the CRT reader over the canvas; closing it
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
    const { session, stop } = startDemo(canvas, sink, {
      forceRgba8: params.get("bloom") === "rgba8",
      openFluid,
      place: placeFor(params),
      props: params.get("props") !== "0",
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
