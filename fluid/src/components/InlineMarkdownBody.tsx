/**
 * The inline renderer itself, in a module of its own so it can be a chunk of
 * its own. `InlineMarkdown.tsx` next door is what a screen imports; this is
 * what that one loads when there is a line of prose to draw.
 *
 * `allowedElements` with `unwrapDisallowed` is what keeps this inline:
 * anything that is not on the list - a heading, a list, a fenced code block,
 * an image, the paragraph react-markdown always wraps a line in - collapses
 * to its own children instead of being dropped or drawn as a block, so the
 * caller's own wrapper (a `<li>`, typically) is the only block element in
 * play. Raw HTML stays inert the same way it does in the full renderer:
 * react-markdown does not interpret it unless something adds `rehype-raw`,
 * which this file does not.
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

export default function InlineMarkdownBody({ source }: { source: string }) {
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
