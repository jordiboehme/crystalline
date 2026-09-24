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
 * The HUD is written straight into the DOM through refs, not through React
 * state: the frame time changes four times a second and a React render of
 * the shell for each would cost more than the number is worth.
 */

import { useEffect, useRef, useState } from "react";
import { Link } from "react-router";

import { detectEnvironment, refusalReason, type Refusal } from "../device";
import { hasWebGL2 } from "../gl/context";
import { startDemo } from "./demo";

const C64_BLUE = "#352879";
const C64_LIGHT_BLUE = "#6c5eb5";

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
 * up on a device that is about to be refused. The engine starts in an
 * effect once the canvas exists, and the effect's cleanup is `startDemo`'s.
 */
export default function LookDemo() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const statusRef = useRef<HTMLSpanElement>(null);
  const frameRef = useRef<HTMLSpanElement>(null);
  const noticeRef = useRef<HTMLDivElement>(null);
  const [refusal] = useState<Refusal | null>(() => {
    const params = new URLSearchParams(window.location.search);
    if (params.has("nogl")) return "no-webgl2";
    return refusalReason(detectEnvironment(hasWebGL2));
  });

  useEffect(() => {
    const canvas = canvasRef.current;
    if (refusal !== null || canvas === null) return;
    const params = new URLSearchParams(window.location.search);
    return startDemo(
      canvas,
      {
        status: (text) => {
          if (statusRef.current) statusRef.current.textContent = text;
        },
        frame: (text) => {
          if (frameRef.current) frameRef.current.textContent = text;
        },
        notice: (text) => {
          if (noticeRef.current) {
            noticeRef.current.textContent = text ?? "";
            noticeRef.current.hidden = text === null;
          }
        },
      },
      { forceRgba8: params.get("bloom") === "rgba8" },
    );
  }, [refusal]);

  if (refusal !== null) return <DeviceRefusal />;
  return (
    <div className="fixed inset-0 bg-black">
      <canvas
        ref={canvasRef}
        className="block h-full w-full cursor-crosshair"
      />
      <div className="pointer-events-none absolute inset-x-0 top-0 flex justify-between gap-4 p-3 font-mono text-xs text-white/85 [text-shadow:0_1px_2px_black]">
        <span>
          STATION LOOK DEMO · 1 DAY SHIFT · 2 APERTURE GRID · 4 FREESCAPE 64 · R
          RETIRED · WASD ARROWS MOUSE
        </span>
        <span ref={frameRef} />
      </div>
      <div className="pointer-events-none absolute inset-x-0 bottom-0 p-3 font-mono text-xs text-white/85 [text-shadow:0_1px_2px_black]">
        <span ref={statusRef} />
      </div>
      <div
        ref={noticeRef}
        hidden
        className="absolute inset-0 flex items-center justify-center font-mono text-lg text-white"
      />
    </div>
  );
}
