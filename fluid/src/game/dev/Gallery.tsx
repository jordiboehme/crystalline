/**
 * MODEL GALLERY: every model the station draws, in one room, for judging.
 *
 * The room is `galleryRoom()`, built by hand rather than generated from an
 * engram, and shown with `session.showRoom`: a door of every style, the
 * sealed ways, the portals, a terminal, a hatch, a poster and the placard in
 * the hall with every kind of furniture, the twelve machines in bays 1 and
 * 2, one of every prop kind and variant in bays 3 and 4 (the floor kinds
 * spilling from rows 1, 3 and 5 onto row 7 once the four tall kinds no
 * longer fit), and a row of every ceiling span kind and variant over row 6
 * of those same two bays.
 * Development only - the route that renders this exists only under
 * `import.meta.env.DEV`, and stays that way for good.
 *
 * The session has no query client, so walking through a door says
 * `SIGNAL LOST` and leaves the player in the gallery, and F opens nothing:
 * the gallery is no engram. `?bloom=rgba8` forces the RGBA8 bloom path as
 * in the look demo.
 *
 * Two more parameters are dev-only and not on the legend. `?at=<kind>:<n>`
 * (see `spotSpawn`) puts the player in front of the n-th fixture of that
 * kind, facing it, instead of the room's own entrance; use it to judge a
 * malfunctioning fixture without walking across the hall. `?fault=missing`
 * or `?fault=denied` answers every travel with that failure instead of
 * `SIGNAL LOST`, so every open door, portal and hatch in the gallery
 * malfunctions once the player walks into or crawls through it, and stays
 * broken for the rest of the visit. The sealed sliding door (`door:4`) and
 * the sealed portal (`portal:2`) malfunction from the start, with or
 * without `?fault=`.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { detectEnvironment, refusalReason, type Refusal } from "../device";
import { hasWebGL2 } from "../gl/context";
import { createSession, type PlaceLoader, type Session } from "../session";
import { DeviceRefusal } from "../ui/DeviceRefusal";
import { StationView } from "../ui/StationView";
import { useHud } from "../ui/useHud";
import { galleryRoom } from "../world/canned";
import { spotSpawn } from "./spots";

/** The keys, along the top of the screen. */
const LEGEND =
  "MODEL GALLERY · 1 DAY SHIFT · 2 APERTURE GRID · 4 FREESCAPE 64 · WASD ARROWS MOUSE · E USE · I INVERT";

/** F goes nowhere from the gallery: there is no engram to open. */
function openNothing() {}

/**
 * The gallery screen. The device is refused before the first paint as in
 * the look demo, and the session is started in an effect once the canvas
 * exists and disposed in its cleanup.
 */
export default function Gallery() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sessionRef = useRef<Session | null>(null);
  const { sink, view, connector, reader } = useHud();
  const [refusal] = useState<Refusal | null>(() =>
    refusalReason(detectEnvironment(hasWebGL2)),
  );

  useEffect(() => {
    const canvas = canvasRef.current;
    if (refusal !== null || canvas === null) return;
    const params = new URLSearchParams(window.location.search);
    const base = galleryRoom();
    const at = params.get("at");
    const spawn = at === null ? null : spotSpawn(base, at);
    const fault = params.get("fault");
    const load: PlaceLoader | undefined =
      fault === "missing" || fault === "denied"
        ? () => Promise.resolve({ kind: fault })
        : undefined;
    const session = createSession({
      canvas,
      client: null,
      hud: sink,
      navigate: openNothing,
      openFluid: openNothing,
      forceRgba8: params.get("bloom") === "rgba8",
      ...(load === undefined ? {} : { load }),
    });
    sessionRef.current = session;
    session.showRoom(spawn === null ? base : { ...base, spawn });
    return () => {
      sessionRef.current = null;
      session.dispose();
    };
  }, [refusal, sink]);

  const closeReader = useCallback(() => {
    sessionRef.current?.closeReader();
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
      onOpenFluid={openNothing}
    />
  );
}
