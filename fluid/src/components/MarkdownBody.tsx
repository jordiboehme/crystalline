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
 * Wikilinks stay text unless somebody says otherwise. `[[Title]]` is a link
 * only where the API says what it resolves to, which the engram page knows and
 * this renderer does not, so it arrives as the `wikilinks` resolver rather than
 * being invented here from the text inside the brackets. With no resolver, or
 * for a target the resolver says nothing about, the brackets stay prose.
 *
 * Mermaid is loaded only when a document actually draws one. The library is
 * larger than the rest of this app put together, so it lives behind a lazy
 * import and a fence that never appears never costs a byte.
 */

import { Suspense, lazy, useMemo } from "react";
import type { ComponentProps } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import { Link } from "react-router";
import remarkGfm from "remark-gfm";

import { WIKILINK, referenceState } from "../wikilinks";
import type { WikilinkResolution, WikilinkResolver } from "../wikilinks";

const MermaidDiagram = lazy(() => import("./MermaidDiagram"));

/**
 * The renderer's own plugin-list type, taken from its props rather than from
 * the unified packages underneath it, which are a transitive dependency this
 * package does not declare.
 */
type RehypePlugins = ComponentProps<typeof ReactMarkdown>["rehypePlugins"];

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

/**
 * The marker the wikilink rewrite leaves on the elements it makes, so the
 * component map can tell one of its anchors from a markdown link that happens
 * to sit beside it.
 */
const WIKILINK_MARKER = "data-wikilink";

/** Whether this node is one of ours, and which kind. */
function wikilinkKind(node: unknown): string | null {
  const properties = (node as HastNode | undefined)?.properties;
  const marker = properties?.[WIKILINK_MARKER];
  return typeof marker === "string" ? marker : null;
}

/**
 * The subtrees a `[[...]]` is content rather than a reference in: code of
 * either kind, where it is the text somebody wrote about wikilinks, and an
 * existing link, which already points somewhere.
 */
const OPAQUE = new Set(["code", "pre", "a"]);

/**
 * Rewrite the `[[Target]]` occurrences in a document's prose into elements the
 * component map below draws.
 *
 * A rehype pass rather than a remark one because the two outcomes need two
 * different elements, and mdast has a node for a link but none for "text with
 * something to say about it". It runs after the highlighter, whose subtrees it
 * skips anyway.
 */
function wikilinkRewrite(resolve: WikilinkResolver) {
  return function plugin() {
    return function transform(tree: unknown) {
      rewrite(tree as HastNode, resolve);
    };
  };
}

/** Walk one node, replacing the text of every child that is not opaque. */
function rewrite(node: HastNode, resolve: WikilinkResolver): void {
  if (
    !node.children ||
    (node.tagName !== undefined && OPAQUE.has(node.tagName))
  ) {
    return;
  }
  const rewritten: HastNode[] = [];
  for (const child of node.children) {
    if (child.type === "text") {
      rewritten.push(...split(child.value ?? "", resolve));
    } else {
      rewrite(child, resolve);
      rewritten.push(child);
    }
  }
  node.children = rewritten;
}

/**
 * Split one run of text around the wikilinks the resolver recognizes.
 *
 * The three reference states are classified by the one shared rule, and the
 * pending one is drawn here by leaving the text exactly as written: a target
 * the index resolved and the graph has not placed yet is prose, which is what
 * it already was, and it becomes a link once the graph lands without ever
 * having claimed to be broken.
 */
function split(text: string, resolve: WikilinkResolver): HastNode[] {
  const parts: HastNode[] = [];
  let taken = 0;
  for (const match of text.matchAll(WIKILINK)) {
    // WIKILINK's one capture group is not optional, so it is always present
    // in a match; this only documents that guarantee to the checker.
    const target = match[1];
    if (target === undefined) {
      continue;
    }
    const resolution = resolve(target);
    // Bracket text carries no parsed reference of its own, so the payload has
    // nothing to say about it here.
    const state = referenceState(resolution, null);
    if (state === "pending" || resolution === null) {
      continue;
    }
    if (match.index > taken) {
      parts.push({ type: "text", value: text.slice(taken, match.index) });
    }
    parts.push(element(resolution, match[0]));
    taken = match.index + match[0].length;
  }
  if (taken === 0) {
    return [{ type: "text", value: text }];
  }
  if (taken < text.length) {
    parts.push({ type: "text", value: text.slice(taken) });
  }
  return parts;
}

/** The element one recognized wikilink becomes. */
function element(resolution: WikilinkResolution, literal: string): HastNode {
  if (resolution.kind === "resolved") {
    return {
      type: "element",
      tagName: "a",
      properties: { href: resolution.href, [WIKILINK_MARKER]: "resolved" },
      // The label rather than the literal: the brackets were the source's way
      // of marking a reference, and a link marks it now.
      children: [{ type: "text", value: resolution.label }],
    };
  }
  return {
    type: "element",
    tagName: "span",
    properties: { [WIKILINK_MARKER]: "unresolved" },
    // Left exactly as written, so a reader can see what the engram claims
    // points somewhere.
    children: [{ type: "text", value: literal }],
  };
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
  a: ({ children, href, node }) => {
    // A resolved wikilink points at a screen of this app, so it navigates
    // in place rather than reloading it. The route was built by the resolver
    // from the graph's own addresses, so there is nothing left to validate.
    if (wikilinkKind(node) === "resolved" && typeof href === "string") {
      return (
        <Link
          to={href}
          className="text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
        >
          {children}
        </Link>
      );
    }
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
  span: ({ children, node }) =>
    // The only spans in this tree are the wikilink rewrite's own: raw HTML
    // stays text here, so nothing in an engram can write one.
    wikilinkKind(node) === "unresolved" ? (
      <span
        title="not resolved"
        className="underline decoration-dotted underline-offset-2 opacity-70"
      >
        {children}
      </span>
    ) : (
      <span>{children}</span>
    ),
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

export default function MarkdownBody({
  source,
  wikilinks,
}: {
  source: string;
  wikilinks?: WikilinkResolver;
}) {
  const rehypePlugins = useMemo<RehypePlugins>(
    () => [
      // `plainText` keeps the highlighter's hands off a mermaid fence, whose
      // text this file reads back out to draw the diagram from.
      [rehypeHighlight, { plainText: ["mermaid"] }],
      // After the highlighter, so what it built is already in place. The
      // rewrite skips code either way.
      ...(wikilinks ? [wikilinkRewrite(wikilinks)] : []),
    ],
    [wikilinks],
  );
  return (
    <div className="text-[0.95rem]">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={rehypePlugins}
        components={components}
      >
        {source.replace(FRONTMATTER, "")}
      </ReactMarkdown>
    </div>
  );
}
