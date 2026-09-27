/**
 * The heads-up display: everything drawn over the station besides the room.
 *
 * The prompt for what the player faces sits at the bottom centre on a
 * dark backing fitted to its text (`PROMPT_BACKING`), so it stays readable
 * over whatever lies behind it, the white doorway of an open police box
 * included (2.6e C28); the status line (room, look, condition, mouse hint) at
 * the bottom left, the frame time at the top right beside an optional key
 * legend, and a notice
 * (`ACCESS DENIED`, `LOOK INVERTED`) in the middle. Over all of them, while
 * travelling, the connector.
 *
 * `useHud` (in `useHud.ts`) makes the `HudSink` the session writes to and
 * the callback refs this component binds its text lines with: those lines
 * are written straight into the DOM, never rendered through React state.
 * The connector is React state the host passes in; the host mounts the CRT
 * reader itself.
 */

import { Connector, type ConnectorState } from "./Connector";
import type { HudView } from "./useHud";

/**
 * How dark the prompt's backing is: black at this opacity, the class
 * `bg-black/60` the prompt carries. White text over it keeps a contrast of
 * at least 4.5:1 even with a white surface behind (WCAG 2's AA level for
 * body text), which a text shadow alone does not.
 */
export const PROMPT_BACKING = 0.6;

/** The props of `Hud`. */
export interface HudProps {
  view: HudView;
  connector: ConnectorState;
  /** A key legend for the top left, or nothing. */
  legend?: string;
}

/**
 * The HUD's elements over a full-screen canvas: the text lines the sink
 * writes and the connector. None of it takes the mouse, so a click reaches
 * the canvas and asks for pointer lock.
 */
export function Hud({ view, connector, legend }: HudProps) {
  const { prompt, status, frame, notice } = view;
  const line =
    "pointer-events-none font-mono text-xs text-white/85 [text-shadow:0_1px_2px_black]";
  return (
    <>
      <div
        className={`absolute inset-x-0 top-0 flex justify-between gap-4 p-3 ${line}`}
      >
        <span>{legend ?? ""}</span>
        <span ref={frame} />
      </div>
      <div className={`absolute inset-x-0 bottom-0 p-3 ${line}`}>
        <span ref={status} />
      </div>
      <div
        ref={prompt}
        hidden
        className="pointer-events-none absolute bottom-12 left-1/2 -translate-x-1/2 whitespace-nowrap rounded-sm bg-black/60 px-3 py-1 text-center font-mono text-base tracking-wider text-white [text-shadow:0_1px_3px_black]"
      />
      <div
        ref={notice}
        hidden
        className="pointer-events-none absolute inset-0 flex items-center justify-center font-mono text-lg text-white [text-shadow:0_1px_3px_black]"
      />
      <Connector {...connector} />
    </>
  );
}
