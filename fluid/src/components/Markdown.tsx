/**
 * Markdown, rendered in the browser. What every screen with something to read
 * imports.
 *
 * It is a seam rather than the renderer: the renderer and its syntax
 * highlighter are the heaviest dependencies in this app after mermaid, and the
 * screen the app opens on draws no markdown at all. Behind a lazy import they
 * are a chunk that arrives with the first document rather than bytes every
 * visit pays for. Measured on this app at the time it was split: 368 kB raw,
 * 112 kB gzipped, which was half the entry bundle.
 *
 * The rules that shape what the renderer will and will not draw live with it,
 * in `MarkdownBody.tsx`.
 */

import { Suspense, lazy } from "react";

import type { WikilinkResolver } from "../wikilinks";

const MarkdownBody = lazy(() => import("./MarkdownBody"));

export interface MarkdownProps {
  /** The markdown as written, frontmatter and all. */
  source: string;
  /**
   * What each `[[Target]]` in the prose resolves to, when the caller knows.
   * Absent, and for every target it answers `null` about, a wikilink stays the
   * text it was written as: only the engram page holds the payloads that say
   * where one goes.
   */
  wikilinks?: WikilinkResolver;
  /**
   * The title the caller has already drawn as a page heading, when it has.
   * The body's own opening `# Title` folds away where it repeats this one, so
   * the screen says it once; absent, every heading in the document is drawn.
   */
  foldTitle?: string;
}

export function Markdown({ source, wikilinks, foldTitle }: MarkdownProps) {
  return (
    <Suspense
      fallback={
        // Deliberately quiet rather than the raw source: the chunk is one
        // request and showing unrendered markdown first would flash. The
        // gem facet-fills while the renderer chunk loads.
        <p className="flex items-center gap-2 py-3 font-mono text-sm text-slate-500 dark:text-slate-400">
          <span aria-hidden className="gem-cycle inline-grid">
            <span>{"◇"}</span>
            <span>{"◈"}</span>
            <span>{"◆"}</span>
          </span>
          crystallizing
        </p>
      }
    >
      <MarkdownBody
        source={source}
        {...(wikilinks ? { wikilinks } : {})}
        {...(foldTitle === undefined ? {} : { foldTitle })}
      />
    </Suspense>
  );
}
