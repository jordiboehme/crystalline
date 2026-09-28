/**
 * The gem in the top bar, the easter egg living behind it, and the station's
 * way in.
 *
 * Triple-click the mark (or hold it for a moment on touch) and it fractures
 * into four shards that fly apart; what reassembles is a C64 boot screen
 * carrying the credits. Both triggers are deliberate accidents-cannot-happen
 * shapes: no state outside this component, and a single or double click
 * still navigates home exactly as before.
 *
 * The screen itself is a small love letter: border and phosphor colors are
 * the VICE palette, the RAM line counts engram bytes, and the links are LOAD
 * commands with reverse-video hover, the way the real machine highlighted
 * text. It renders through a portal so no anchor ever nests inside the
 * header's home link.
 *
 * Three LOAD lines are listed: the source, the coffee, and `LOAD"GAME",8,1`,
 * which launches the station at `/π` (M4 C1, C3). The same command can be
 * typed at the cursor: printable keys are read by `event.key` (a German
 * layout sends the `"` as Shift+2, so never by `code`), upper-cased and
 * echoed after `READY.`, Backspace takes one back, and a line holds at most
 * `LINE_CAP` characters. Enter runs a non-empty line: the command, spaces
 * removed, launches; anything else answers `?SYNTAX  ERROR` and `READY.`.
 * The screen's key listener is the one global listener here, and only while
 * the screen is open. It runs in the capture phase and stops every key it
 * consumes, so the app's own window shortcuts (`?` for help, `\` for the
 * width) never act behind the screen, and an Enter that runs a typed line
 * never also follows a focused link. Enter on an empty line, Esc, Tab and
 * any key held with Ctrl, Cmd or Alt are left alone. The typed line and the
 * `?SYNTAX  ERROR` line are polite live regions, so a screen reader hears
 * the typing and the answer.
 *
 * A launch starts from the page the screen was opened on, not from where
 * the triple click leaves the app: its first two clicks are ordinary clicks
 * on the home link and navigate home. So the gem remembers the router's
 * location on the first click (`event.detail === 1`, whose handler runs
 * before the home link's) and when a long press fires, and the screen falls
 * back to the current location only when none was remembered. The router's
 * location, never `window.location`, which an in-memory router never moves.
 * The launch primes the station's sound inside the click or key that
 * launches (`primeAudio`, M4 C4), since some browsers start sound only in a
 * user gesture. `game/launch.ts` is the one game module imported here, and
 * it imports nothing, so the main chunk carries no more of the game.
 */

import type { PointerEvent, MouseEvent, ReactElement } from "react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Link, useLocation, useNavigate } from "react-router";

import { useAuth } from "../auth/AuthContext";
import { gamePathOf, primeAudio } from "../game/launch";
import { GemGlyph } from "./GemGlyph";

const REPO_URL = "https://github.com/jordiboehme/crystalline";
const SUPPORT_URL = "https://ko-fi.com/V7V31T6CL9";
const LONG_PRESS_MS = 600;
const SHATTER_MS = 550;

/** The command that launches the station, typed or listed. */
export const LAUNCH_COMMAND = 'LOAD"GAME",8,1';

/** The machine's answer to any other typed line, with its two spaces. */
export const SYNTAX_ERROR = "?SYNTAX  ERROR";

/** The most characters one typed line holds. */
export const LINE_CAP = 40;

/** The reverse-video highlight every LOAD line shares. */
const LOAD_CLASSES =
  "hover:bg-[#7c70da] hover:text-[#40318d] focus:bg-[#7c70da] focus:text-[#40318d] focus:outline-none";

type Phase = "idle" | "shattering" | "about";

/** A Fluid location, the part a launch carries into the station. */
interface Origin {
  pathname: string;
  search: string;
}

/** A LOAD command that is secretly a link. */
function LoadLine({ href, label }: { href: string; label: string }) {
  return (
    <p>
      <a href={href} target="_blank" rel="noreferrer" className={LOAD_CLASSES}>
        {`LOAD"${label}",8,1`}
      </a>
    </p>
  );
}

/** The LOAD line that launches the station, a router link primed on click. */
function GameLine({ to }: { to: string }) {
  return (
    <p>
      <Link to={to} onClick={primeAudio} className={LOAD_CLASSES}>
        {LAUNCH_COMMAND}
      </Link>
    </p>
  );
}

function C64Screen({
  onClose,
  origin,
}: {
  onClose: () => void;
  origin: Origin | null;
}) {
  const { capabilities } = useAuth();
  const here = useLocation();
  const navigate = useNavigate();
  const screenRef = useRef<HTMLDivElement>(null);
  const [typed, setTyped] = useState("");
  const [answer, setAnswer] = useState<string | null>(null);
  const from = origin ?? here;
  const target = gamePathOf(from.pathname, from.search);

  // The listener reads the latest line and callbacks through a ref, so it
  // is registered once for the screen's life and a keystroke never
  // re-registers it or moves the focus.
  const latest = useRef({ typed, onClose, navigate, target });
  useEffect(() => {
    latest.current = { typed, onClose, navigate, target };
  });

  useEffect(() => {
    screenRef.current?.focus();
  }, []);

  useEffect(() => {
    const consume = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
    };
    const onKey = (event: KeyboardEvent) => {
      const now = latest.current;
      if (event.key === "Escape") {
        now.onClose();
        return;
      }
      if (event.ctrlKey || event.metaKey || event.altKey) {
        return;
      }
      if (event.key === "Enter") {
        if (now.typed === "") {
          return;
        }
        consume(event);
        if (now.typed.replaceAll(" ", "") === LAUNCH_COMMAND) {
          primeAudio();
          void now.navigate(now.target);
          return;
        }
        setAnswer(now.typed);
        setTyped("");
        now.typed = "";
        return;
      }
      if (event.key === "Backspace") {
        consume(event);
        now.typed = now.typed.slice(0, -1);
        setTyped(now.typed);
        return;
      }
      if (event.key.length === 1) {
        consume(event);
        now.typed = (now.typed + event.key.toUpperCase()).slice(0, LINE_CAP);
        setTyped(now.typed);
      }
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () =>
      window.removeEventListener("keydown", onKey, { capture: true });
  }, []);

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/60 p-4"
      role="presentation"
      onClick={onClose}
    >
      <div
        ref={screenRef}
        role="dialog"
        aria-modal="true"
        aria-label="About Crystalline"
        tabIndex={-1}
        className="plaque-in w-full max-w-lg rounded-sm bg-[#7c70da] p-6 shadow-2xl outline-none sm:p-10"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="relative bg-[#40318d] px-4 py-5 font-mono text-sm leading-6 text-[#7c70da]">
          <div
            aria-hidden
            className="crt-scan pointer-events-none absolute inset-0"
          />
          <div className="whitespace-pre-wrap">
            <p className="text-center">
              {`**** CRYSTALLINE V${capabilities.serverVersion.toUpperCase()} ****`}
            </p>
            <p className="mt-1 text-center">
              {"64K RAM SYSTEM  38911 ENGRAM BYTES FREE"}
            </p>
            <p className="mt-5">CONCEIVED AND GROWN BY JORDI BÖHME.</p>
            <p>EST.2025. FLUID IS WHERE FLUID AND</p>
            <p>CRYSTALLIZED INTELLIGENCE MEET.</p>
            <p className="mt-5">READY.</p>
            <LoadLine href={REPO_URL} label="SOURCE" />
            <LoadLine href={SUPPORT_URL} label="COFFEE" />
            <GameLine to={target} />
            {answer !== null && (
              <>
                <p className="mt-2">{answer}</p>
                <p aria-live="polite">{SYNTAX_ERROR}</p>
                <p>READY.</p>
              </>
            )}
            <p aria-live="polite" className={answer === null ? "mt-2" : ""}>
              {typed}
              <span aria-hidden className="crystal-cursor">
                {"█"}
              </span>
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="relative mt-4 font-mono text-xs text-[#7c70da] hover:bg-[#7c70da] hover:text-[#40318d] focus:bg-[#7c70da] focus:text-[#40318d] focus:outline-none"
          >
            RUN/STOP (ESC)
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

export function ShatterGem(): ReactElement {
  const [phase, setPhase] = useState<Phase>("idle");
  const pressTimer = useRef<number | null>(null);
  const suppressClick = useRef(false);
  const { pathname, search } = useLocation();
  // Where the screen was opened from: set on the first click of a triple
  // click and when a long press fires, before any navigation home.
  const [origin, setOrigin] = useState<Origin | null>(null);

  useEffect(() => {
    if (phase !== "shattering") {
      return;
    }
    const settle = window.setTimeout(() => setPhase("about"), SHATTER_MS);
    return () => window.clearTimeout(settle);
  }, [phase]);

  useEffect(
    () => () => {
      if (pressTimer.current !== null) {
        window.clearTimeout(pressTimer.current);
      }
    },
    [],
  );

  const clearPress = () => {
    if (pressTimer.current !== null) {
      window.clearTimeout(pressTimer.current);
      pressTimer.current = null;
    }
  };

  const onPointerDown = (event: PointerEvent) => {
    if (event.pointerType === "mouse" && event.button !== 0) {
      return;
    }
    clearPress();
    pressTimer.current = window.setTimeout(() => {
      pressTimer.current = null;
      suppressClick.current = true;
      setOrigin({ pathname, search });
      setPhase("shattering");
    }, LONG_PRESS_MS);
  };

  const onClick = (event: MouseEvent) => {
    if (suppressClick.current) {
      suppressClick.current = false;
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (event.detail === 1) {
      setOrigin({ pathname, search });
    }
    if (event.detail >= 3) {
      event.preventDefault();
      event.stopPropagation();
      if (phase === "idle") {
        setPhase("shattering");
      }
    }
  };

  return (
    <span
      className="relative inline-flex"
      onClick={onClick}
      onPointerDown={onPointerDown}
      onPointerUp={clearPress}
      onPointerLeave={clearPress}
      onPointerCancel={clearPress}
      onContextMenu={(event) => {
        if (pressTimer.current !== null || suppressClick.current) {
          event.preventDefault();
        }
      }}
    >
      <span className={phase === "idle" ? "" : "opacity-0"}>
        <GemGlyph />
      </span>
      {phase === "shattering" && (
        <span aria-hidden className="gem-shards">
          <span>
            <GemGlyph />
          </span>
          <span>
            <GemGlyph />
          </span>
          <span>
            <GemGlyph />
          </span>
          <span>
            <GemGlyph />
          </span>
        </span>
      )}
      {phase === "about" && (
        <C64Screen origin={origin} onClose={() => setPhase("idle")} />
      )}
    </span>
  );
}
