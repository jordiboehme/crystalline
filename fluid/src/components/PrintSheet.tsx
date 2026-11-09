/**
 * The parts of a printed page that the screen never shows and every reading
 * page shares: the title block at the top of the first page and the running
 * header and footer. The Related section of an engram and the Notes of a
 * Reflection page are modules of their own (`PrintRelated`, `PrintNotes`).
 *
 * Each block carries the `hidden` attribute and `data-print`: hidden on
 * screen and from assistive technology there, and shown by the print sheet
 * in `index.css`. The reading pages reach it through `PageActions`, beside
 * the Print view row, so it rides in the chunk they already share and none
 * of it reaches the eager bundle.
 *
 * The running header and footer are `@page` margin boxes. A margin box
 * cannot read the document, so the words go into a small style element
 * here. Chrome draws them (131 and later); Safari and Firefox do not draw
 * margin boxes, and there the title block carries the address and the print
 * date on its own.
 */

import type { ReactElement, RefObject } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { encode } from "uqr";

import type { PrintFact } from "../printSheet";
import { cssString, qrPath } from "../printSheet";

/** Today as a short date in the reader's own locale (`Oct 16, 2026` in en-US). */
function today(): string {
  return new Date().toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
  });
}

/**
 * Run `start` as printing starts and `end` as it ends: on `beforeprint` and
 * `afterprint`, and on the print media query, which a browser that prints
 * without those events (a PDF made by a driver) still flips. Returns the
 * unsubscribe.
 */
function whilePrinting(start: () => void, end?: () => void): () => void {
  const media =
    typeof window.matchMedia === "function" ? window.matchMedia("print") : null;
  const after = () => end?.();
  const onChange = (event: MediaQueryListEvent) => {
    if (event.matches) {
      start();
    } else {
      after();
    }
  };
  window.addEventListener("beforeprint", start);
  window.addEventListener("afterprint", after);
  media?.addEventListener("change", onChange);
  return () => {
    window.removeEventListener("beforeprint", start);
    window.removeEventListener("afterprint", after);
    media?.removeEventListener("change", onChange);
  };
}

/**
 * The page's pictures load before it prints: one the reader never scrolled
 * to is still `loading="lazy"`, and a printer must not get its empty box.
 * They go back to lazy after printing.
 */
function useEagerImagesWhilePrinting(): void {
  useEffect(() => {
    let lazy: HTMLImageElement[] = [];
    const start = () => {
      if (lazy.length > 0) {
        return;
      }
      lazy = [
        ...document.querySelectorAll<HTMLImageElement>(
          'main img[loading="lazy"]',
        ),
      ];
      for (const image of lazy) {
        image.setAttribute("loading", "eager");
      }
    };
    const end = () => {
      for (const image of lazy) {
        image.setAttribute("loading", "lazy");
      }
      lazy = [];
    };
    const stop = whilePrinting(start, end);
    return () => {
      stop();
      end();
    };
  }, []);
}

/**
 * Words with the day the page is printed in them. The day is read again as
 * printing starts and written straight into the element that shows it: a
 * re-render would land after the browser has laid out the printed page. So
 * a tab left open overnight prints today's date, not the day it was opened.
 */
function usePrintDate<T extends HTMLElement>(
  words: (date: string) => string,
): { ref: RefObject<T | null>; text: string } {
  const [date] = useState(today);
  const ref = useRef<T>(null);
  const latest = useRef(words);
  useEffect(() => {
    latest.current = words;
  });
  useEffect(
    () =>
      whilePrinting(() => {
        if (ref.current !== null) {
          ref.current.textContent = latest.current(today());
        }
      }),
    [],
  );
  return { ref, text: words(date) };
}

/**
 * The running header and footer, in the browsers that draw `@page` margin
 * boxes: where the page comes from and its title at the top, the address,
 * the print date and the page number at the bottom. Their look is in
 * `index.css`.
 */
export function PrintPageStyle({
  source,
  title,
  address,
}: {
  /** Where the page comes from: the domain, and for a Reflection page "Reflection". */
  source: string;
  title: string;
  address: string;
}): ReactElement {
  // The first page opens with the title block, which says all of this once:
  // only its page number stays. Here rather than in `index.css`, where a
  // first-page rule loses to these later ones.
  useEagerImagesWhilePrinting();
  const { ref, text } = usePrintDate<HTMLStyleElement>(
    (date) => `@media print {
@page {
  @top-left { content: ${cssString(source)}; }
  @top-right { content: ${cssString(title)}; }
  @bottom-left { content: ${cssString(address)}; }
  @bottom-right { content: ${cssString(`Printed ${date} \u00b7 `)} counter(page) " of " counter(pages); }
}
@page :first {
  @top-left { content: ""; }
  @top-right { content: ""; }
  @bottom-left { content: ""; }
  @bottom-right { content: counter(page) " of " counter(pages); }
}
}`,
  );
  return (
    <style ref={ref} data-print-page="">
      {text}
    </style>
  );
}

/**
 * The top of the first printed page: where the page comes from, its title,
 * its description, its facts, and its address with the print date.
 */
export function PrintTitleBlock({
  trail,
  title,
  lead = null,
  facts = [],
  address,
  webAddress,
}: {
  /** Domain and folders, drawn small above the title. */
  trail: string[];
  /** Null when the page's own text opens with its title. */
  title: string | null;
  lead?: string | null;
  facts?: PrintFact[];
  /** The page's name: its `crystalline://` address, or its web address. */
  address: string;
  /** The page's web address, which the QR stamp in the corner opens. */
  webAddress: string;
}): ReactElement {
  const { ref: printedRef, text: printed } = usePrintDate<HTMLSpanElement>(
    (date) => `Printed ${date}`,
  );
  return (
    <header
      hidden
      data-print=""
      className="print-title mb-1 border-b border-hair-strong pb-4"
    >
      {/*
        The stamp keeps a column of its own at the right, so the title never
        runs under it and never wraps around it.
      */}
      <div className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-6">
        <div className="min-w-0">
          <p className="text-caption font-medium tracking-[0.08em] text-muted uppercase">
            {trail.join(" / ")}
          </p>
          {title !== null && <p className="mt-1.5 text-display">{title}</p>}
          {lead !== null && (
            <p className="mt-2 text-lg leading-snug text-ink-3">{lead}</p>
          )}
          {facts.length > 0 && (
            <dl className="mt-3.5 flex flex-wrap gap-x-6 gap-y-1 text-sm">
              {facts.map((fact) => (
                <div key={fact.label} className="flex gap-1.5">
                  <dt className="text-muted">{fact.label}</dt>
                  <dd className="font-medium text-ink-2">{fact.value}</dd>
                </div>
              ))}
            </dl>
          )}
        </div>
        <QrStamp text={webAddress} />
      </div>
      <p className="mt-2 flex flex-wrap justify-between gap-x-4 text-caption text-muted">
        <span className="font-mono break-all">{address}</span>
        <span ref={printedRef}>{printed}</span>
      </p>
    </header>
  );
}

/**
 * A small stamp at the top right of the first printed page: the page's web
 * address as a QR code (error correction M, a quiet zone of four modules),
 * black on white, about 22 mm square, with a caption. The encoder rides in
 * this lazy chunk only.
 */
function QrStamp({ text }: { text: string }): ReactElement {
  // Encoding an address takes well under a millisecond, so it is done with
  // the page rather than at the last moment before printing.
  const data = useMemo(
    () => encode(text, { ecc: "M", border: 4 }).data,
    [text],
  );
  const size = data.length;
  // One link around the code and its caption: a PDF made from the print keeps
  // it, so a reader can click the stamp instead of scanning it.
  return (
    <figure className="print-qr w-[22mm]">
      <a
        href={text}
        className="flex flex-col items-center gap-1 text-inherit no-underline"
      >
        <svg
          role="img"
          aria-label={`QR code: ${text}`}
          data-qr-text={text}
          data-qr-size={size}
          viewBox={`0 0 ${String(size)} ${String(size)}`}
          width="22mm"
          height="22mm"
          shapeRendering="crispEdges"
        >
          <rect width={size} height={size} fill="#ffffff" />
          <path d={qrPath(data)} fill="#000000" />
        </svg>
        <span className="text-[7pt] leading-none text-muted">
          Open in Fluid
        </span>
      </a>
    </figure>
  );
}
