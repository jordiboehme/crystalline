/**
 * The markdown renderer itself, in a module of its own so it can be a chunk of
 * its own. `Markdown.tsx` next door is what the app imports; this is what that
 * one loads when a screen actually has markdown to draw.
 *
 * The API is a data API and never returns HTML, so this is where an engram or a
 * MANIFEST becomes something to read. Three rules shape it.
 *
 * Raw HTML stays text. react-markdown does not interpret HTML unless somebody
 * adds `rehype-raw`, and nobody may: the markdown is whatever was written into
 * the knowledge base, and a `<script>` in an engram is a string, not a
 * capability. `Markdown.test.tsx` is the tripwire on that.
 *
 * Wikilinks stay text too. `[[Title]]` is a link only where the API says what
 * it resolves to, which is the engram page; here it is prose, and inventing a
 * route from the text inside the brackets would produce links that go nowhere.
 *
 * Mermaid is loaded only when a document actually draws one. The library is
 * larger than the rest of this app put together, so it lives behind a lazy
 * import and a fence that never appears never costs a byte.
 */

import { Suspense, lazy } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";

const MermaidDiagram = lazy(() => import("./MermaidDiagram"));

/**
 * The frontmatter block at the top of every engram and every MANIFEST.
 *
 * It is data rather than prose - the frontmatter panel and the domain header
 * are what present it - and left in it would render as a heading made of a
 * `title:` line, which is nobody's intent. Anchored at the start of the
 * document, so a thematic break further down is untouched.
 */
const FRONTMATTER = /^---\r?\n[\s\S]*?\r?\n---[ \t]*(\r?\n|$)/;

/**
 * A hast node, narrowed to what this file reads off one.
 *
 * Declared here rather than imported from `hast`: those types are a transitive
 * dependency of react-markdown rather than one this package declares, so
 * importing them would reach through the dependency tree.
 */
interface HastNode {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
  value?: string;
}

/** Every character inside a node, in order. */
function textOf(node: HastNode | undefined): string {
  if (!node) {
    return "";
  }
  if (node.type === "text") {
    return node.value ?? "";
  }
  return (node.children ?? []).map(textOf).join("");
}

/** The language a fenced code block declared, or null when it declared none. */
function languageOf(node: HastNode | undefined): string | null {
  const names = node?.properties?.className;
  if (!Array.isArray(names)) {
    return null;
  }
  for (const name of names) {
    if (typeof name === "string" && name.startsWith("language-")) {
      return name.slice("language-".length);
    }
  }
  return null;
}

/**
 * How each element is drawn.
 *
 * Written out rather than pulled from a typography plugin: the app owns a
 * handful of colors and one dark variant, and a plugin would bring a second
 * opinion about all of them.
 */
const components: Components = {
  h1: ({ children }) => (
    <h1 className="mt-6 mb-3 text-2xl font-semibold first:mt-0">{children}</h1>
  ),
  h2: ({ children }) => (
    <h2 className="mt-6 mb-3 text-xl font-semibold first:mt-0">{children}</h2>
  ),
  h3: ({ children }) => (
    <h3 className="mt-5 mb-2 text-lg font-semibold first:mt-0">{children}</h3>
  ),
  h4: ({ children }) => (
    <h4 className="mt-4 mb-2 font-semibold first:mt-0">{children}</h4>
  ),
  p: ({ children }) => <p className="my-3 leading-relaxed">{children}</p>,
  ul: ({ children }) => (
    <ul className="my-3 list-disc pl-6 leading-relaxed">{children}</ul>
  ),
  ol: ({ children }) => (
    <ol className="my-3 list-decimal pl-6 leading-relaxed">{children}</ol>
  ),
  li: ({ children }) => <li className="my-1">{children}</li>,
  a: ({ children, href }) => {
    // react-markdown's own URL transform has already dropped the protocols a
    // link may not carry (`javascript:` among them). What is left to decide is
    // where it opens: a link out of the app leaves this tab alone and gets no
    // handle on it, while an anchor or an in-app path navigates in place.
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
  blockquote: ({ children }) => (
    <blockquote className="my-3 border-l-2 border-slate-300 pl-4 text-slate-600 dark:border-slate-700 dark:text-slate-300">
      {children}
    </blockquote>
  ),
  hr: () => <hr className="my-6 border-slate-200 dark:border-slate-800" />,
  table: ({ children }) => (
    // Wide tables scroll inside themselves rather than widening the page.
    <div className="my-4 overflow-x-auto">
      <table className="w-full border-collapse text-sm">{children}</table>
    </div>
  ),
  th: ({ children }) => (
    <th className="border-b border-slate-300 px-3 py-1.5 text-left font-semibold dark:border-slate-700">
      {children}
    </th>
  ),
  td: ({ children }) => (
    <td className="border-b border-slate-200 px-3 py-1.5 align-top dark:border-slate-800">
      {children}
    </td>
  ),
  code: ({ children, className }) => {
    // A fenced block carries a language class or the highlighter's own, and
    // sits inside a `pre` that is already the box. Only the inline kind gets
    // drawn as a chip.
    const fenced =
      typeof className === "string" &&
      (className.includes("language-") || className.includes("hljs"));
    return fenced ? (
      <code className={className}>{children}</code>
    ) : (
      <code className="rounded bg-slate-100 px-1 py-0.5 font-mono text-[0.9em] dark:bg-slate-800">
        {children}
      </code>
    );
  },
  pre: ({ children, node }) => {
    const fence = (node as HastNode | undefined)?.children?.[0];
    const source = textOf(fence);
    if (languageOf(fence) === "mermaid" && source.trim() !== "") {
      return (
        <figure
          aria-label="Diagram"
          className="my-4 overflow-x-auto rounded border border-slate-200 p-3 dark:border-slate-800"
        >
          <Suspense fallback={<DiagramSource source={source} />}>
            <MermaidDiagram source={source} />
          </Suspense>
        </figure>
      );
    }
    return (
      <pre className="my-4 overflow-x-auto rounded bg-slate-100 p-3 text-sm dark:bg-slate-900">
        {children}
      </pre>
    );
  },
};

/** A diagram's source, which is what shows while the renderer is on its way. */
function DiagramSource({ source }: { source: string }) {
  return (
    <pre className="overflow-x-auto font-mono text-xs text-slate-500 dark:text-slate-400">
      {source}
    </pre>
  );
}

export default function MarkdownBody({ source }: { source: string }) {
  return (
    <div className="text-[0.95rem]">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        // `plainText` keeps the highlighter's hands off a mermaid fence, whose
        // text this file reads back out to draw the diagram from.
        rehypePlugins={[[rehypeHighlight, { plainText: ["mermaid"] }]]}
        components={components}
      >
        {source.replace(FRONTMATTER, "")}
      </ReactMarkdown>
    </div>
  );
}
