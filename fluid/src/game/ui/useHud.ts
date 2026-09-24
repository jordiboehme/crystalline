/**
 * The HUD's sink: where the session writes what the player sees besides
 * the room, made once per host.
 *
 * The four text lines (prompt, status, frame time, notice) are written
 * straight into the DOM, not through React state: the frame time changes
 * four times a second and the prompt whenever the player turns, and a React
 * render of the shell for each would cost more than the text is worth.
 * `Hud` hands its elements over through callback refs (`HudView`), and the
 * sink writes each one's `textContent`, so a title or heading from an
 * engram is shown as the characters it is. The connector and the reader
 * change once per journey or terminal, so they are React state for the
 * host to render.
 */

import { useEffect, useState } from "react";

import type { LookId } from "../render/looks";
import type { HudSink, ReaderState } from "../session";
import { CONNECTOR_MIN_MS, type ConnectorState } from "./Connector";

/**
 * The callback refs `Hud` binds its four text lines with: each receives
 * its element on mount and null on unmount.
 */
export interface HudView {
  prompt: (el: HTMLElement | null) => void;
  status: (el: HTMLElement | null) => void;
  frame: (el: HTMLElement | null) => void;
  notice: (el: HTMLElement | null) => void;
}

/** What `useHud` gives its host. */
export interface HudBindings {
  /** The sink to hand the session; the same object for the host's lifetime. */
  sink: HudSink;
  /** The refs to hand `Hud`; the same object for the host's lifetime. */
  view: HudView;
  /** The connector as the sink last set it, held up for its minimum. */
  connector: ConnectorState;
  /** The reader's content while a terminal is read, else null. */
  reader: ReaderState | null;
}

/** Sets an element's text, or empties and hides it for null. */
function write(el: HTMLElement | null, text: string | null) {
  if (el === null) return;
  el.textContent = text ?? "";
  el.hidden = text === null;
}

/**
 * Makes the HUD's sink, its view refs and its state.
 *
 * The sink and the view are made once, so a session created in an effect
 * keeps writing to the same elements, and text written before an element
 * mounts is shown when it does. Hiding the connector is delayed until it
 * has been up for `CONNECTOR_MIN_MS`; a pending hide is dropped when the
 * host unmounts.
 */
export function useHud(initialLook: LookId = "aperture"): HudBindings {
  const [connector, setConnector] = useState<ConnectorState>({
    active: false,
    label: "",
    look: initialLook,
  });
  const [reader, setReader] = useState<ReaderState | null>(null);

  const [made] = useState(() => {
    type Line = keyof HudView;
    const els: Record<Line, HTMLElement | null> = {
      prompt: null,
      status: null,
      frame: null,
      notice: null,
    };
    // What each line should show, kept so an element that mounts after the
    // session wrote to it still shows the latest text.
    const texts: Record<Line, string | null> = {
      prompt: null,
      status: "",
      frame: "",
      notice: null,
    };
    const set = (line: Line, text: string | null) => {
      texts[line] = text;
      write(els[line], text);
    };
    const bind = (line: Line) => (el: HTMLElement | null) => {
      els[line] = el;
      write(el, texts[line]);
    };
    let shownAt = 0;
    let hideTimer: ReturnType<typeof setTimeout> | null = null;
    const cancelHide = () => {
      if (hideTimer !== null) clearTimeout(hideTimer);
      hideTimer = null;
    };
    const sink: HudSink = {
      prompt: (text) => {
        set("prompt", text);
      },
      status: (text) => {
        set("status", text);
      },
      frame: (text) => {
        set("frame", text);
      },
      notice: (text) => {
        set("notice", text);
      },
      connector: (active, label, look) => {
        cancelHide();
        if (active) {
          shownAt = performance.now();
          setConnector({ active: true, label, look });
          return;
        }
        const left = CONNECTOR_MIN_MS - (performance.now() - shownAt);
        if (left <= 0) {
          setConnector({ active: false, label, look });
          return;
        }
        hideTimer = setTimeout(() => {
          hideTimer = null;
          setConnector({ active: false, label, look });
        }, left);
      },
      reader: setReader,
    };
    const view: HudView = {
      prompt: bind("prompt"),
      status: bind("status"),
      frame: bind("frame"),
      notice: bind("notice"),
    };
    return { sink, view, cancelHide };
  });

  useEffect(() => made.cancelHide, [made]);

  return { sink: made.sink, view: made.view, connector, reader };
}
