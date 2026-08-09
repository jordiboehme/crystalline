/**
 * The three utility actions on an engram page. Download hands over the
 * detail payload's `content`, which IS the exact file text: the server reads
 * the file with no normalization and checksums those very bytes, so a Blob
 * of it is the file, byte for byte, without a raw-bytes route existing.
 * Share copies the page's own URL - the browser-shaped address, where Copy
 * address next to it copies the crystalline:// name. Print leans on the
 * print stylesheet: chrome carries print:hidden, so what prints is the
 * content, the title and the small metadata line.
 */

import type { ReactElement } from "react";
import { useEffect, useState } from "react";

import type { EngramDetail } from "../api/engram";
import { engramRoute } from "../paths";

/** How long the confirmations stay up, matching CopyAddressButton. */
const CONFIRMED_FOR_MS = 2000;

/**
 * The download filename: the permalink's last segment plus .md.
 *
 * Exported alongside the component rather than split into a second file:
 * splitting one small pure helper out for a lint rule alone would scatter
 * this file's whole exported surface for no real benefit (same call made in
 * `editor/FindingsPanel.tsx`).
 */
// eslint-disable-next-line react-refresh/only-export-components
export function downloadName(permalink: string): string {
  const slug = permalink.split("/").at(-1) ?? permalink;
  return `${slug}.md`;
}

const BUTTON_CLASSES =
  "rounded border border-slate-300 px-2 py-0.5 text-xs hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800";

export interface EngramActionsProps {
  engram: EngramDetail;
}

export function EngramActions({ engram }: EngramActionsProps): ReactElement {
  const [said, setSaid] = useState<string | null>(null);
  useEffect(() => {
    if (said === null) {
      return;
    }
    const timer = setTimeout(() => {
      setSaid(null);
    }, CONFIRMED_FOR_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [said]);

  return (
    <span className="inline-flex items-center gap-2 print:hidden">
      <button
        type="button"
        className={BUTTON_CLASSES}
        onClick={() => {
          const blob = new Blob([engram.content], { type: "text/markdown" });
          const href = URL.createObjectURL(blob);
          const anchor = document.createElement("a");
          anchor.href = href;
          anchor.download = downloadName(engram.permalink);
          anchor.click();
          URL.revokeObjectURL(href);
        }}
      >
        Download as Markdown
      </button>
      <button
        type="button"
        className={BUTTON_CLASSES}
        onClick={() => {
          void (async () => {
            try {
              // `navigator.clipboard` is absent on an insecure or older
              // context, and reading `.writeText` off it throws
              // synchronously rather than rejecting a promise - the same
              // reason CopyAddressButton wraps its call in try/catch rather
              // than chaining `.then`/`.catch` off it directly.
              const link = `${window.location.origin}${engramRoute(engram.domain, engram.permalink)}`;
              await navigator.clipboard.writeText(link);
              setSaid("Link copied");
            } catch {
              setSaid("Copy refused");
            }
          })();
        }}
      >
        Share link
      </button>
      <button
        type="button"
        className={BUTTON_CLASSES}
        onClick={() => {
          window.print();
        }}
      >
        Print view
      </button>
      <span
        role="status"
        aria-live="polite"
        aria-label="Share link result"
        className="text-xs text-slate-500 dark:text-slate-400"
      >
        {said ?? ""}
      </span>
    </span>
  );
}
