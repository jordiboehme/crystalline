/**
 * The connector: the full-screen overlay the player sees while travelling
 * from one room to the next.
 *
 * A band of diagonal hazard stripes wipes across the screen and the
 * destination is spelled out under it, `CONNECTING` and the place's title,
 * in the look the station is drawn in: black and yellow for the day shift,
 * cyan on black for the aperture grid, and the C64's light blue on blue for
 * the 8-bit filled-polygon look. It covers the canvas while the next room is
 * loaded and generated, so the player never sees a half-built room or the
 * old room with the new one's URL.
 *
 * It is shown for at least `CONNECTOR_MIN_MS`: a room that is already in the
 * cache loads in a frame or two, and an overlay that flashes up for that
 * long reads as a glitch rather than a journey. The minimum is kept by the
 * HUD sink (`useHud`), which delays hiding it; this component only draws
 * what it is given. The label is the engram's title, drawn as a text node,
 * except for the level select's jump, which names the domain instead (the
 * session's `go` call takes a label of its own there, C10).
 */

import type { LookId } from "../render/looks";

/** The shortest time the connector stays up once shown, in milliseconds. */
export const CONNECTOR_MIN_MS = 400;

/** What the connector shows: whether it is up, where to, and in which look. */
export interface ConnectorState {
  active: boolean;
  label: string;
  look: LookId;
}

/** Each look's colours: the stripes' two colours and the text. */
const STYLES: Record<LookId, { a: string; b: string; bg: string; fg: string }> =
  {
    day: { a: "#f2c200", b: "#111111", bg: "#111111", fg: "#f2c200" },
    aperture: { a: "#16e0ff", b: "#05080a", bg: "#05080a", fg: "#16e0ff" },
    freescape: { a: "#6c5eb5", b: "#352879", bg: "#352879", fg: "#6c5eb5" },
  };

/** The keyframes of the hazard wipe, sliding the stripes across. */
const WIPE_CSS = `@keyframes station-connector-wipe {
  from { background-position: 0 0; }
  to { background-position: 113px 0; }
}`;

/**
 * The overlay, or nothing while the connector is down. It sits above the
 * HUD and takes no mouse input, so a click still reaches the canvas.
 */
export function Connector({ active, label, look }: ConnectorState) {
  if (!active) return null;
  const style = STYLES[look];
  return (
    <div
      role="status"
      aria-live="polite"
      className="pointer-events-none fixed inset-0 z-40 flex flex-col items-center justify-center gap-6 font-mono uppercase"
      style={{ background: style.bg, color: style.fg }}
    >
      <style>{WIPE_CSS}</style>
      <div
        aria-hidden="true"
        className="h-16 w-full"
        style={{
          backgroundImage: `repeating-linear-gradient(45deg, ${style.a} 0px, ${style.a} 40px, ${style.b} 40px, ${style.b} 80px)`,
          animation: "station-connector-wipe 0.6s linear infinite",
        }}
      />
      <p className="text-2xl tracking-widest">CONNECTING</p>
      <p className="max-w-[80vw] truncate text-lg">{label}</p>
      <div
        aria-hidden="true"
        className="h-16 w-full"
        style={{
          backgroundImage: `repeating-linear-gradient(-45deg, ${style.a} 0px, ${style.a} 40px, ${style.b} 40px, ${style.b} 80px)`,
          animation: "station-connector-wipe 0.6s linear infinite reverse",
        }}
      />
    </div>
  );
}
