/**
 * One line of prose with its emphasis and inline code drawn, rather than
 * shown as literal markdown punctuation - a MANIFEST bullet, for instance,
 * where `**Search `project2030` first**` should read as bold text around a
 * code span rather than asterisks and backticks.
 *
 * It is a seam rather than the renderer, the same split `Markdown.tsx` makes
 * for the block renderer and for the same reason: react-markdown is a real
 * dependency, and a routing bullet drawn on a page that never opens a
 * document should not pay for it up front. Measured at the point this split
 * was made: the block renderer's own chunk already carries react-markdown,
 * so a page that draws both - a domain's MANIFEST facets beside its raw
 * source, for instance - fetches it once either way; bundling this renderer
 * into the entry chunk instead grew it by roughly 117 kB raw (36 kB gzipped).
 * No loading flash needed here, unlike the block renderer's "crystallizing"
 * placeholder: the fallback is the plain source text, which for the common
 * case - a bullet with no markdown in it at all - is pixel-identical to the
 * rendered answer, and for the rare bullet that does use markup is a one-time
 * flash of its own punctuation rather than a blank line.
 */

import { Suspense, lazy } from "react";

const InlineMarkdownBody = lazy(() => import("./InlineMarkdownBody"));

export interface InlineMarkdownProps {
  /** The single line of prose to draw. */
  source: string;
}

/** One line of prose, its markdown emphasis and inline code drawn rather than
 * left as punctuation. No block elements, ever: a heading or a list written
 * into the source loses only its own wrapper, never the text inside it. */
export function InlineMarkdown({ source }: InlineMarkdownProps) {
  return (
    <Suspense fallback={source}>
      <InlineMarkdownBody source={source} />
    </Suspense>
  );
}
