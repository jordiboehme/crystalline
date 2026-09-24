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
 * The HUD is `ui/Hud.tsx`, the game's own: its text lines are written
 * straight into the DOM through refs, not through React state, because the
 * frame time changes four times a second and a React render of the shell
 * for each would cost more than the number is worth.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { Link } from "react-router";

import { engramRoute } from "../../paths";
import { detectEnvironment, refusalReason, type Refusal } from "../device";
import { hasWebGL2 } from "../gl/context";
import type { Session } from "../session";
import { CrtReader } from "../ui/CrtReader";
import { Hud } from "../ui/Hud";
import { useHud } from "../ui/useHud";
import { startDemo } from "./demo";

const C64_BLUE = "#352879";
const C64_LIGHT_BLUE = "#6c5eb5";

/** The keys, along the top of the screen. */
const LEGEND =
  "STATION LOOK DEMO · 1 DAY SHIFT · 2 APERTURE GRID · 4 FREESCAPE 64 · R RETIRED · WASD ARROWS MOUSE · E USE · F FLUID · I INVERT";

/** Opens a Fluid page in a new tab, as the F key does in the game. */
function openFluid(path: string) {
  window.open(path, "_blank", "noopener");
}

/**
 * The C64's answer to a device that cannot run the station: the error in
 * light blue on the dark blue screen, and RUN/STOP back to the app.
 */
function DeviceRefusal() {
  return (
    <div
      className="fixed inset-0 flex items-center justify-center p-8 font-mono text-lg uppercase"
      style={{ background: C64_LIGHT_BLUE }}
    >
      <div
        className="w-full max-w-2xl p-8"
        style={{ background: C64_BLUE, color: C64_LIGHT_BLUE }}
      >
        <p>?DEVICE NOT PRESENT ERROR</p>
        <p>READY.</p>
        <p className="mt-6">
          <Link to="/" className="underline">
            RUN/STOP
          </Link>
        </p>
      </div>
    </div>
  );
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
    <div className="fixed inset-0 bg-black">
      <canvas
        ref={canvasRef}
        className="block h-full w-full cursor-crosshair"
      />
      <Hud view={view} connector={connector} legend={LEGEND} />
      {reader !== null && (
        <CrtReader
          key={`${reader.section?.heading ?? ""}\u0000${String(reader.section?.occurrence ?? 0)}`}
          title={reader.title}
          markdown={reader.content}
          section={reader.section}
          look={reader.look === "freescape" ? "petscii" : "phosphor"}
          onClose={closeReader}
          onOpenFluid={readerOpenFluid}
        />
      )}
    </div>
  );
}
