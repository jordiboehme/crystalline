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

import type { ReactNode } from "react";

import type { AssetResolver } from "../api/files";
import type { WikilinkResolver } from "../wikilinks";
import { Loading } from "./Loading";
import { lazyPart } from "./lazyPart";
import type { HeadingAside, MarkdownAnchors } from "./MarkdownBody";

const MarkdownBody = lazyPart(() => import("./MarkdownBody"));

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
  /**
   * The domain this document lives in. It is what a relative `assets/` target
   * resolves against - an attachment path is domain-relative and means nothing
   * without one - so a caller that knows says so, and one that does not leaves
   * such targets as the text they were written as.
   */
  domain?: string;
  /**
   * What a relative link target resolves against and where its bytes are
   * read, for a document whose files are not its domain's engram files. It
   * wins over {@link domain}.
   */
  assets?: AssetResolver;
  /**
   * What this document is called, for the name on a file a reader takes out
   * of it: a diagram or an image saved from the full window is named after
   * the document it came from. Absent, such a download is named after the
   * picture alone.
   */
  documentName?: string;
  /**
   * The page this document is read at, which is what makes its headings
   * addressable: every heading gets an id, a link symbol beside it copies the
   * page URL plus that id, and a fragment in the location scrolls to the
   * heading it names. Absent, none of that is drawn - a surface nobody can
   * link into should not offer to be linked into.
   */
  anchors?: MarkdownAnchors;
  /**
   * Stamp positioned text with its source offset, for comments on selected
   * text. Only the reflection page sets it.
   */
  sourceSpans?: boolean;
  /** A line drawn under the first `# Title`, outside the text (see `MarkdownBody`). */
  titleNote?: ReactNode;
  /** A control at the right end of the first heading, outside the text. */
  headingAside?: HeadingAside;
  /**
   * Flip one task box: the offset of its `[` in `source`, and what it
   * becomes. Absent, the boxes only show their state.
   */
  onTick?: (offset: number, next: "[ ]" | "[x]") => void;
  /** Whether the boxes tick, or a sentence saying why not. */
  canTick?: boolean | string;
}

export function Markdown(props: MarkdownProps) {
  return (
    <MarkdownBody
      fallback={
        // Deliberately quiet rather than the raw source: the chunk is one
        // request and showing unrendered markdown first would flash.
        <Loading label="Loading the text" subject="the text" />
      }
      // Handed on whole: every prop means the same to the body, and one
      // spread costs the eager bundle less than a line per prop.
      {...props}
    />
  );
}
