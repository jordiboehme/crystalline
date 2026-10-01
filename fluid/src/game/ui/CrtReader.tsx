/**
 * The CRT reader: what a terminal, a machine, a poster, the placard or a
 * screen shows when the player leans in with Space.
 *
 * A full-screen overlay laid out as an 80-column terminal, the engram's
 * markdown flattened by `crtLines` into screen lines. The font is sized so
 * the 80 columns fit the viewport, and the screen shows as many whole lines
 * as fit under it; scrolling moves the first visible line, one line or one
 * screen at a time, never past the last full screen unless the section it
 * opened at lies in that screen. The screen is the
 * phosphor green of a 70s terminal on near-black. Headings
 * are drawn in reverse video; the scanlines are a CSS repeating gradient and
 * the vignette a radial one, both laid over the text and ignoring the mouse.
 *
 * It opens at the reading's section (a terminal's own): the
 * `occurrence`-th line index the layout records for the heading, or the
 * top when there is none or the text no longer has that section. The keys are read on `window` by `KeyboardEvent.code` while
 * the reader is mounted, with the page's default action prevented so W, S
 * and the page keys do not also scroll the page: W/S or the arrows move one
 * line, Page Up/Down a screen, the wheel scrolls, F opens the engram in
 * Fluid and Esc closes. A key pressed with Ctrl, Cmd or Alt is left to the
 * browser. Pointer lock is released on open, so the keys and
 * the wheel reach the page instead of the game's locked canvas.
 *
 * The engram is data, never markup: every line is a React text child, so an
 * `<img onerror>` in the prose is shown as the characters it is.
 */

import { useEffect, useMemo, useRef, useState } from "react";

import { CRT_COLUMNS, crtLines, type CrtLine } from "./crt";

/** The screen's colours. */
const PALETTE = {
  fg: "#39ff7a",
  bg: "#050b07",
  glow: "0 0 4px #39ff7a88",
} as const;

/** The footer every screen ends with. */
const FOOTER = "W/S SCROLL  F OPEN IN FLUID  ESC CLOSE";

/** The margin around the screen, in pixels, on every side. */
const MARGIN = 16;

/** The largest font the screen grows to, in pixels. */
const MAX_FONT = 22;

/**
 * The font size, line height and number of text rows for a viewport: the
 * font is as large as lets 80 columns fit the width (a monospace character
 * is about 0.6 of the font size wide), and the rows are what fits the height
 * once the title and footer lines are taken off.
 */
function screenMetrics(width: number, height: number) {
  const font = Math.max(
    8,
    Math.min(MAX_FONT, Math.floor((width - 2 * MARGIN) / (CRT_COLUMNS * 0.6))),
  );
  const lineHeight = Math.round(font * 1.25);
  const rows = Math.max(1, Math.floor((height - 2 * MARGIN) / lineHeight) - 2);
  return { font, lineHeight, rows };
}

/** How one kind of line is drawn beyond its text. */
function lineStyle(
  kind: CrtLine["kind"],
  palette: typeof PALETTE,
): React.CSSProperties {
  switch (kind) {
    case "heading":
      return { background: palette.fg, color: palette.bg, textShadow: "none" };
    case "quote":
    case "note":
      return { opacity: 0.75 };
    case "code":
      return { opacity: 0.9 };
    default:
      return {};
  }
}

/** The props of the reader. */
export interface CrtReaderProps {
  /** The engram's title, shown in the screen's top line. */
  title: string;
  /** The engram's markdown, frontmatter included or not. */
  markdown: string;
  /**
   * The section to open at (a terminal's): its `##` heading as written and
   * how many sections of the same heading come before it. Null opens at the
   * top. It is read once, at mount: the parent mounts a fresh reader for each
   * reading (a new `key`), so a later change of this prop moves nothing.
   */
  section: { heading: string; occurrence: number } | null;
  /** Called on F: open the engram in Fluid. */
  onOpenFluid: () => void;
  /** Called on Esc: leave the reader. */
  onClose: () => void;
}

/**
 * The reader overlay. See the module doc for the layout and the keys; the
 * parent mounts it while a fixture is being read and unmounts it on
 * `onClose`.
 */
export function CrtReader({
  title,
  markdown,
  section,
  onOpenFluid,
  onClose,
}: CrtReaderProps) {
  const layout = useMemo(() => crtLines(markdown), [markdown]);
  const [viewport, setViewport] = useState(() => ({
    width: window.innerWidth,
    height: window.innerHeight,
  }));
  const { font, lineHeight, rows } = screenMetrics(
    viewport.width,
    viewport.height,
  );
  // The section's first line, decided once at mount. The scroll limit keeps
  // the last screen full, but never stops short of the section, so a section
  // in the last screen still opens at the top with empty rows below it, as a
  // terminal would show it.
  const [sectionStart] = useState(() =>
    section === null
      ? 0
      : (layout.sections.get(section.heading)?.[section.occurrence] ?? 0),
  );
  const maxTop = Math.max(0, layout.lines.length - rows, sectionStart);
  const [top, setTop] = useState(sectionStart);
  const first = Math.min(Math.max(0, top), maxTop);
  const wheelRest = useRef(0);

  useEffect(() => {
    if (document.pointerLockElement) document.exitPointerLock();
  }, []);

  useEffect(() => {
    const onResize = () => {
      setViewport({ width: window.innerWidth, height: window.innerHeight });
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
    };
  }, []);

  useEffect(() => {
    const scroll = (by: number) => {
      setTop((t) => Math.min(Math.max(0, Math.min(t, maxTop) + by), maxTop));
    };
    const onKey = (event: KeyboardEvent) => {
      // Cmd+F, Ctrl+W and the like keep their browser meaning.
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      switch (event.code) {
        case "KeyW":
        case "ArrowUp":
          scroll(-1);
          break;
        case "KeyS":
        case "ArrowDown":
          scroll(1);
          break;
        case "PageUp":
          scroll(-rows);
          break;
        case "PageDown":
          scroll(rows);
          break;
        case "KeyF":
          if (!event.repeat) onOpenFluid();
          break;
        case "Escape":
          if (!event.repeat) onClose();
          break;
        default:
          return;
      }
      event.preventDefault();
    };
    // The wheel's delta comes in pixels, lines or pages depending on the
    // device; it is summed in pixels and turned into whole lines, so a
    // trackpad's many small deltas scroll as smoothly as a notched wheel.
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const unit =
        event.deltaMode === 1
          ? lineHeight
          : event.deltaMode === 2
            ? rows * lineHeight
            : 1;
      wheelRest.current += event.deltaY * unit;
      const lines = Math.trunc(wheelRest.current / lineHeight);
      if (lines !== 0) {
        wheelRest.current -= lines * lineHeight;
        scroll(lines);
      }
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("wheel", onWheel);
    };
  }, [rows, maxTop, lineHeight, onOpenFluid, onClose]);

  const palette = PALETTE;
  const visible = layout.lines.slice(first, first + rows);
  const position =
    layout.lines.length === 0
      ? "0/0"
      : `${String(first + 1)}/${String(layout.lines.length)}`;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={title}
      className="fixed inset-0 z-50 flex items-center justify-center overflow-hidden font-mono"
      style={{ background: palette.bg, color: palette.fg }}
    >
      <div
        className="relative whitespace-pre"
        style={{
          width: `${String(CRT_COLUMNS)}ch`,
          fontSize: `${String(font)}px`,
          lineHeight: `${String(lineHeight)}px`,
          textShadow: palette.glow,
        }}
      >
        <div
          className="flex justify-between gap-[1ch]"
          style={{ height: lineHeight }}
        >
          <span className="min-w-0 truncate">{title}</span>
          <span className="shrink-0">{position}</span>
        </div>
        <div data-testid="crt-lines" style={{ height: rows * lineHeight }}>
          {visible.map((line, n) => (
            <div
              key={first + n}
              style={{ height: lineHeight, ...lineStyle(line.kind, palette) }}
            >
              {line.text}
            </div>
          ))}
        </div>
        <div style={{ height: lineHeight }}>{FOOTER}</div>
      </div>
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0"
        style={{
          backgroundImage:
            "repeating-linear-gradient(to bottom, rgba(0, 0, 0, 0.28) 0px, rgba(0, 0, 0, 0.28) 1px, transparent 1px, transparent 3px)",
        }}
      />
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0"
        style={{
          background:
            "radial-gradient(ellipse at center, transparent 60%, rgba(0, 0, 0, 0.45) 100%)",
        }}
      />
    </div>
  );
}
