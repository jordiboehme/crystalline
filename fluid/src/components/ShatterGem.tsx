/**
 * The gem in the top bar, and the easter egg living behind it.
 *
 * Triple-click the mark (or hold it for a moment on touch) and it fractures
 * into four shards that fly apart; what reassembles is a C64 boot screen
 * carrying the credits. Both triggers are deliberate accidents-cannot-happen
 * shapes: no global key listeners, no state outside this component, and a
 * single or double click still navigates home exactly as before.
 *
 * The screen itself is a small love letter: border and phosphor colors are
 * the VICE palette, the RAM line counts engram bytes, and the links are LOAD
 * commands with reverse-video hover, the way the real machine highlighted
 * text. It renders through a portal so no anchor ever nests inside the
 * header's home link.
 */

import { Gem } from "lucide-react";
import type { PointerEvent, MouseEvent, ReactElement } from "react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { useAuth } from "../auth/AuthContext";

const REPO_URL = "https://github.com/jordiboehme/crystalline";
const SUPPORT_URL = "https://ko-fi.com/V7V31T6CL9";
const LONG_PRESS_MS = 600;
const SHATTER_MS = 550;

type Phase = "idle" | "shattering" | "about";

/** One gem drawing, shared by the mark and its shards. */
function GemGlyph(): ReactElement {
  return (
    <Gem
      aria-hidden="true"
      size={18}
      strokeWidth={1.75}
      className="text-accent-600 dark:text-accent-400"
    />
  );
}

/** A LOAD command that is secretly a link. */
function LoadLine({ href, label }: { href: string; label: string }) {
  return (
    <p>
      <a
        href={href}
        target="_blank"
        rel="noreferrer"
        className="hover:bg-[#7c70da] hover:text-[#40318d] focus:bg-[#7c70da] focus:text-[#40318d] focus:outline-none"
      >
        {`LOAD"${label}",8,1`}
      </a>
    </p>
  );
}

function C64Screen({ onClose }: { onClose: () => void }) {
  const { capabilities } = useAuth();
  const screenRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    screenRef.current?.focus();
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

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
            <p className="mt-2" aria-hidden>
              <span className="crystal-cursor">{"█"}</span>
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
      {phase === "about" && <C64Screen onClose={() => setPhase("idle")} />}
    </span>
  );
}
