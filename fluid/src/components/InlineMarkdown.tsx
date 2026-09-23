/**
 * One line of prose with its emphasis and inline code drawn, rather than
 * shown as literal markdown punctuation - a MANIFEST bullet, for instance,
 * where `**Search `project2030` first**` should read as bold text around a
 * code span rather than asterisks and backticks.
 *
 * A seam of its own rather than a mode on `Markdown`: that component is lazy
 * because the renderer behind it carries a syntax highlighter and mermaid,
 * and a line of routing prose needs neither and should never show a loading
 * flash for two words of bold text. `allowedElements` with `unwrapDisallowed`
 * is what keeps this inline: anything that is not on the list - a heading, a
 * list, a fenced code block, an image, the paragraph react-markdown always
 * wraps a line in - collapses to its own children instead of being dropped or
 * drawn as a block, so the caller's own wrapper (a `<li>`, typically) is the
 * only block element in play. Raw HTML stays inert the same way it does in
 * the full renderer: react-markdown does not interpret it unless something
 * adds `rehype-raw`, which this file does not.
 */

import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";

/** Every tag this renderer is allowed to draw. Everything else unwraps into
 * its own children rather than becoming an element of its own. */
const ALLOWED_ELEMENTS = ["strong", "em", "code", "a"];

/** The inline elements that need more than their default drawing: the same
 * chip-like code span and link styling the block renderer uses, kept in step
 * by hand since the two never share a chunk. */
const components: Components = {
  code: ({ children }) => (
    <code className="rounded bg-slate-100 px-1 py-0.5 font-mono text-[0.9em] dark:bg-slate-800">
      {children}
    </code>
  ),
  a: ({ children, href }) => {
    // The same rule the block renderer's anchor applies to an ordinary link:
    // a target out of the app opens in its own tab, an in-app one navigates
    // in place. There is no wikilink or attachment resolution here - a
    // routing bullet carries neither - so every link is drawn as written.
    const outward = typeof href === "string" && /^https?:\/\//i.test(href);
    return (
      <a
        href={href}
        className="text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
        {...(outward ? { target: "_blank", rel: "noreferrer" } : {})}
      >
        {children}
      </a>
    );
  },
};

export interface InlineMarkdownProps {
  /** The single line of prose to draw. */
  source: string;
}

/** One line of prose, its markdown emphasis and inline code drawn rather than
 * left as punctuation. No block elements, ever: a heading or a list written
 * into the source loses only its own wrapper, never the text inside it. */
export function InlineMarkdown({ source }: InlineMarkdownProps) {
  return (
    <ReactMarkdown
      allowedElements={ALLOWED_ELEMENTS}
      unwrapDisallowed
      components={components}
    >
      {source}
    </ReactMarkdown>
  );
}
