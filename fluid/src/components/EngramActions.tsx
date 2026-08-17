/**
 * The three utility actions on an engram page. Download hands over the
 * detail payload's `content`, which IS the exact file text: the server reads
 * the file with no normalization and checksums those very bytes, so a Blob
 * of it is the file, byte for byte, without a raw-bytes route existing.
 * Share copies the page's own URL - the browser-shaped address, where Copy
 * address on the details panel copies the crystalline:// name. Print leans on
 * the print stylesheet: chrome carries print:hidden, so what prints is the
 * content, the title and the trail above it.
 *
 * This component draws no controls of its own. The three are handed out
 * through the optional `handlers` ref and run from the icon strip in the
 * page's header and from the command palette, which is what keeps one copy of
 * each: the clipboard call has an outcome to announce, and the live region that
 * announces it is here, so a share run from either place says the same thing.
 */

import type { ReactElement, RefObject } from "react";
import { useCallback, useEffect, useImperativeHandle, useState } from "react";

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

/** The three, handed out so the menu and the palette can run them. */
export interface EngramActionHandlers {
  download: () => void;
  share: () => void;
  print: () => void;
}

export interface EngramActionsProps {
  engram: EngramDetail;
  /**
   * Filled in with the handlers the menu rows and the palette run.
   *
   * A ref rather than a second copy of the three bodies in the caller: the
   * clipboard call has a confirmation to announce, and the live region that
   * announces it is here. Two copies would mean a palette share that copied
   * the link and said nothing.
   */
  handlers?: RefObject<EngramActionHandlers | null>;
}

export function EngramActions({
  engram,
  handlers,
}: EngramActionsProps): ReactElement {
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

  const download = useCallback(() => {
    const blob = new Blob([engram.content], { type: "text/markdown" });
    const href = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = href;
    anchor.download = downloadName(engram.permalink);
    anchor.click();
    URL.revokeObjectURL(href);
  }, [engram.content, engram.permalink]);

  const share = useCallback(() => {
    void (async () => {
      try {
        // `navigator.clipboard` is absent on an insecure or older context,
        // and reading `.writeText` off it throws synchronously rather than
        // rejecting a promise - the same reason CopyAddressButton wraps its
        // call in try/catch rather than chaining `.then`/`.catch` off it
        // directly.
        const link = `${window.location.origin}${engramRoute(engram.domain, engram.permalink)}`;
        await navigator.clipboard.writeText(link);
        setSaid("Link copied");
      } catch {
        setSaid("Copy refused");
      }
    })();
  }, [engram.domain, engram.permalink]);

  const print = useCallback(() => {
    window.print();
  }, []);

  useImperativeHandle(handlers, () => ({ download, share, print }), [
    download,
    share,
    print,
  ]);

  // The region is in the document from the start and empty, so the text
  // arriving in it is what gets read out. Nothing else is drawn: the controls
  // that run these live in the page's overflow menu.
  return (
    <span
      role="status"
      aria-live="polite"
      aria-label="Share link result"
      className="text-caption text-slate-500 print:hidden dark:text-slate-400"
    >
      {said ?? ""}
    </span>
  );
}
