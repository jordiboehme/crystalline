/**
 * The three utility actions on an engram page. Download hands over the
 * detail payload's `content`, which IS the exact file text: the server reads
 * the file with no normalization and checksums those very bytes, so a Blob
 * of it is the file, byte for byte, without a raw-bytes route existing.
 * Share copies the page's own URL - the browser-shaped address, where Copy
 * address on the details panel copies the crystalline:// name. It is the
 * address the server spelled for this reader where the payload carries one,
 * so what a person hands over and what an agent hands over are one string,
 * and this browser's own origin where the server could not say. Print leans on
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
import { useCallback, useImperativeHandle } from "react";

import type { EngramDetail } from "../api/engram";
import { engramRoute } from "../paths";
import { saveBlob } from "./downloads";
import { useSaid } from "./useSaid";

/**
 * The document's own name: the permalink's last segment.
 *
 * It names more than this file's download now - a diagram or an image a
 * reader takes out of the document is named after the document it came from,
 * and this is where that name is decided.
 *
 * Exported alongside the component rather than split into a second file:
 * splitting one small pure helper out for a lint rule alone would scatter
 * this file's whole exported surface for no real benefit (same call made in
 * `editor/FindingsPanel.tsx`).
 */
// eslint-disable-next-line react-refresh/only-export-components
export function documentSlug(permalink: string): string {
  return permalink.split("/").at(-1) ?? permalink;
}

/** The download filename: the document's own name plus .md. */
// eslint-disable-next-line react-refresh/only-export-components
export function downloadName(permalink: string): string {
  return `${documentSlug(permalink)}.md`;
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
  const [said, say] = useSaid();

  const download = useCallback(() => {
    // The same hand-over the full window's downloads use, so there is one
    // copy of the object-URL dance in this app rather than three.
    saveBlob(
      new Blob([engram.content], { type: "text/markdown" }),
      downloadName(engram.permalink),
    );
  }, [engram.content, engram.permalink]);

  const share = useCallback(() => {
    void (async () => {
      try {
        // `navigator.clipboard` is absent on an insecure or older context,
        // and reading `.writeText` off it throws synchronously rather than
        // rejecting a promise - the same reason CopyAddressButton wraps its
        // call in try/catch rather than chaining `.then`/`.catch` off it
        // directly.
        // The server's own spelling of this page's address where it could
        // work one out - the same string the tools hand an agent - and the
        // browser's own origin where it could not.
        const link =
          engram.webUrl ??
          `${window.location.origin}${engramRoute(engram.domain, engram.permalink)}`;
        await navigator.clipboard.writeText(link);
        say("Link copied");
      } catch {
        say("Copy refused");
      }
    })();
  }, [engram.domain, engram.permalink, engram.webUrl, say]);

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
