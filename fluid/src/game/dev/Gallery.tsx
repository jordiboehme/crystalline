/**
 * MODEL GALLERY: every model the station draws, in one room, for judging.
 *
 * The room is `galleryRoom()`, built by hand rather than generated from an
 * engram, and shown with `session.showRoom`: a door of every style, the
 * sealed ways, the portals, a terminal, a hatch, a poster and the placard in
 * the hall with every kind of furniture, and the twelve machines in the two
 * bays east of it. Development only - the route that renders this exists
 * only under `import.meta.env.DEV`, and stays that way for good.
 *
 * The session has no query client, so walking through a door says
 * `SIGNAL LOST` and leaves the player in the gallery, and F opens nothing:
 * the gallery is no engram. `?bloom=rgba8` forces the RGBA8 bloom path as
 * in the look demo.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { detectEnvironment, refusalReason, type Refusal } from "../device";
import { hasWebGL2 } from "../gl/context";
import { createSession, type Session } from "../session";
import { DeviceRefusal } from "../ui/DeviceRefusal";
import { StationView } from "../ui/StationView";
import { useHud } from "../ui/useHud";
import { galleryRoom } from "../world/canned";

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
    const session = createSession({
      canvas,
      client: null,
      hud: sink,
      navigate: openNothing,
      openFluid: openNothing,
      forceRgba8: params.get("bloom") === "rgba8",
    });
    sessionRef.current = session;
    session.showRoom(galleryRoom());
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
