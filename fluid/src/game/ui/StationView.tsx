/**
 * The station's screen: the full-screen canvas a session draws on, the HUD
 * over it and, while a terminal is read, the CRT reader over both.
 *
 * The game route and the model gallery host a session the same way and
 * draw the same screen; only what they start the session with differs. So
 * the markup lives here and each host keeps its own effects. The HUD's text
 * lines are written straight into the DOM by the session (`useHud`), so
 * this component renders again only when the connector or the reader
 * changes.
 */

import type { RefObject } from "react";

import type { ReaderState } from "../session";
import type { ConnectorState } from "./Connector";
import { CrtReader } from "./CrtReader";
import { Hud } from "./Hud";
import type { HudView } from "./useHud";

/** The props of `StationView`. */
export interface StationViewProps {
  /** The canvas the host starts its session on. */
  canvasRef: RefObject<HTMLCanvasElement | null>;
  /** From `useHud`: the refs the HUD binds its lines with. */
  view: HudView;
  /** From `useHud`: the connector as the session last set it. */
  connector: ConnectorState;
  /** From `useHud`: the reader's content, or null while none is open. */
  reader: ReaderState | null;
  /** A key legend for the top left, or nothing. */
  legend?: string;
  /** The reader was closed: the host hands the keys back to the session. */
  onCloseReader: () => void;
  /** F in the reader: the host opens the engram's Fluid page. */
  onOpenFluid: () => void;
}

/**
 * The key a reader is mounted under: one per terminal opening. The reader
 * reads its section once, when it mounts, so a reader opened at another
 * terminal, or in another room, must be a new one rather than the old one
 * given new props. Closing a reader unmounts it, so reading the same
 * terminal twice mounts it afresh as well.
 */
function readerKey(reader: ReaderState): string {
  const section = reader.section;
  return [
    reader.title,
    section?.heading ?? "",
    String(section?.occurrence ?? -1),
  ].join("\u0000");
}

/** The canvas, the HUD and the reader. See the module doc. */
export function StationView({
  canvasRef,
  view,
  connector,
  reader,
  legend,
  onCloseReader,
  onOpenFluid,
}: StationViewProps) {
  return (
    <div className="fixed inset-0 bg-black">
      <canvas
        ref={canvasRef}
        className="block h-full w-full cursor-crosshair"
      />
      <Hud
        view={view}
        connector={connector}
        {...(legend === undefined ? {} : { legend })}
      />
      {reader !== null && (
        <CrtReader
          key={readerKey(reader)}
          title={reader.title}
          markdown={reader.content}
          section={reader.section}
          look={reader.look === "freescape" ? "petscii" : "phosphor"}
          onClose={onCloseReader}
          onOpenFluid={onOpenFluid}
        />
      )}
    </div>
  );
}
