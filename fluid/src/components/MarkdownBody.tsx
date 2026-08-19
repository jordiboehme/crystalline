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
 *
 * And the document renders once. A caller that has already drawn the title as
 * a page heading says so through `foldTitle`, and the body's opening `# Title`
 * folds away rather than repeating it; an observation or relation bullet is
 * drawn here with its category or rel type as a chip, so the page has no
 * reason to list the same lines a second time somewhere else.
 */

import { Children, Suspense, isValidElement, lazy, useMemo } from "react";
import type { ComponentProps, ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import { Link } from "react-router";
import remarkGfm from "remark-gfm";

import { attachmentUrl } from "../api/files";
import {
  assetPath,
  decodeTarget,
  imageStyle,
  parseImageFragment,
} from "../editor/imageFormat";
import { WIKILINK, referenceState } from "../wikilinks";
import type { WikilinkResolution, WikilinkResolver } from "../wikilinks";
import { Chip } from "./primitives";

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
 * The document's opening `# Title`.
 *
 * Anchored at the start and ended at the line break, so it is the first line
 * of the document rather than any `#` further down.
 */
const LEADING_H1 = /^\s*#[ \t]+(.+?)[ \t]*(?:\r?\n|$)/;

/**
 * The document's own opening `# Title`, when it repeats the indexed title the
 * page header already renders. Fold it and the page says everything once;
 * an opening H1 that says something ELSE is content and stays.
 *
 * A text-level strip beside the frontmatter one rather than a rule in the
 * component map, because a component is handed one heading at a time and
 * cannot know which one is first.
 */
function foldLeadingTitle(source: string, title: string | undefined): string {
  if (title === undefined) {
    return source;
  }
  const match = LEADING_H1.exec(source);
  if (match && match[1] === title.trim()) {
    return source.slice(match[0].length);
  }
  return source;
}

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

/** An observation line's leading `[category]`, as the parser reads one. */
const OBSERVATION_MARK = /^\[([A-Za-z0-9_-]+)\][ \t]+/;

/** A relation line's type, where its `[[target]]` is still text. */
const RELATION_MARK = /^([A-Za-z][A-Za-z0-9_-]*)[ \t]+(?=\[\[)/;

/** The same type, where the target beside it is already an element. */
const RELATION_BEFORE_ELEMENT = /^([A-Za-z][A-Za-z0-9_-]*)[ \t]+$/;

/** Whether a rendered child is one of the wikilink rewrite's own elements. */
function isWikilink(part: ReactNode): boolean {
  return (
    isValidElement<{ node?: unknown }>(part) &&
    wikilinkKind(part.props.node) !== null
  );
}

/** A marked line: the mark as a chip, then the rest of the line as written. */
function chipped(mark: string, rest: string, parts: ReactNode[]): ReactNode {
  return [
    <Chip mono key="mark">
      {mark}
    </Chip>,
    " ",
    rest,
    ...parts.slice(1),
  ];
}

/**
 * The merged rendering the editor already does, for the reading page: an
 * observation bullet's `[category]` and a relation bullet's rel type become
 * chips in place, and the bullet is the one place the line renders. A bullet
 * shaped like neither is handed back untouched. So is a LOOSE list item
 * (blank-line separated, where the renderer wraps the text in a p element and
 * the item's own children become the line break, that p, another break): its
 * head child is the string "\n", which no mark matches, so it keeps plain
 * rendering - an accepted degradation, since indexed observation and relation
 * bullets are tight single-line list items by construction.
 *
 * A rel type is claimed only where a `[[target]]` follows it, either as the
 * text it was written as or as the element the wikilink rewrite made of it.
 * The engine reads a relation the same way, so a first word before any other
 * element - a link, an emphasis - is prose and stays prose.
 */
function structuredBullet(children: ReactNode): ReactNode {
  const parts = Children.toArray(children);
  const head = parts[0];
  if (typeof head !== "string") {
    return children;
  }
  const observation = OBSERVATION_MARK.exec(head);
  if (observation) {
    return chipped(
      `[${observation[1] ?? ""}]`,
      head.slice(observation[0].length),
      parts,
    );
  }
  const relation =
    RELATION_MARK.exec(head) ??
    (isWikilink(parts[1]) ? RELATION_BEFORE_ELEMENT.exec(head) : null);
  if (relation) {
    // Both shapes end at the whitespace after the type, so what is left of
    // the head is the rest of the line either way - empty, in the second.
    return chipped(relation[1] ?? "", head.slice(relation[0].length), parts);
  }
  return children;
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
  li: ({ children }) => <li className="my-1">{structuredBullet(children)}</li>,
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
    // Wide tables scroll inside themselves rather than widening the page, and
    // keep the full column rather than the reading measure.
    <div className="breakout my-4 overflow-x-auto">
      <table className="w-full border-collapse text-sm">{children}</table>
    </div>
  ),
  // The colons in a table's delimiter row are the only way markdown can say
  // how a column reads, and they reach a cell component as a `style` prop
  // carrying `textAlign` rather than as an `align` attribute, so both kinds of
  // cell hand it straight through. A column that says nothing arrives with no
  // style at all and keeps what it always had: a header reading left from its
  // own class, a body cell with no alignment of its own. Where a column does
  // say something the inline style outranks that class, which is why the
  // header keeps `text-left` rather than making it conditional.
  th: ({ children, style }) => (
    <th
      style={style}
      className="border-b border-slate-300 px-3 py-1.5 text-left font-semibold dark:border-slate-700"
    >
      {children}
    </th>
  ),
  td: ({ children, style }) => (
    <td
      style={style}
      className="border-b border-slate-200 px-3 py-1.5 align-top dark:border-slate-800"
    >
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
          className="breakout my-4 rounded border border-slate-200 p-3 dark:border-slate-800"
        >
          <Suspense fallback={<DiagramSource source={source} />}>
            <MermaidDiagram source={source} />
          </Suspense>
        </figure>
      );
    }
    return (
      <pre className="breakout my-4 overflow-x-auto rounded bg-slate-100 p-3 text-sm dark:bg-slate-900">
        {children}
      </pre>
    );
  },
};

/**
 * The files-route URL a target names, or null when it names no file of this
 * domain - which includes every case where the caller has no domain to resolve
 * against.
 *
 * The single place the reading view turns a written target into an address,
 * asked by the image and the anchor alike, and it asks {@link assetPath} the
 * same question the rail asks so the two can never disagree about a file.
 */
function assetHref(domain: string | undefined, target: string): string | null {
  if (domain === undefined) {
    return null;
  }
  const path = assetPath(target);
  return path === null ? null : attachmentUrl(domain, path);
}

/** The classes every link in a document wears, wherever it points. */
const LINK_CLASSES =
  "text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400";

/**
 * One link, drawn by where it points.
 *
 * Three destinations, in the order they are decided. A resolved wikilink is a
 * screen of this app and navigates in place; the route was built by the
 * resolver from the graph's own addresses, so there is nothing left to
 * validate. An `assets/` target is an attachment of this engram's own domain
 * and points at the files route, in a new tab because what comes back is a pdf
 * or a spreadsheet rather than a page of this app - and whether that opens or
 * downloads is the server's disposition to decide, not this anchor's.
 * Everything else is what it was written as.
 *
 * react-markdown's own URL transform has already dropped the protocols a link
 * may not carry (`javascript:` among them). What is left to decide is where it
 * opens: a link out of the app leaves this tab alone and gets no handle on it,
 * while an anchor or an in-app path navigates in place.
 */
function MarkdownAnchor({
  children,
  href,
  node,
  domain,
}: {
  children?: ReactNode;
  /** Undefined is spelled out: react-markdown hands a link with no target on. */
  href?: string | undefined;
  node?: unknown;
  /** The domain a relative `assets/` target resolves against, when known. */
  domain?: string;
}) {
  if (wikilinkKind(node) === "resolved" && typeof href === "string") {
    return (
      <Link to={href} className={LINK_CLASSES}>
        {children}
      </Link>
    );
  }
  // One question decides it, and the rail asks the same one: does this target
  // name a file of this domain? A `./` prefix is stripped, the fragment is
  // dropped - the files route never sees one - and an address that is somebody
  // else's, or a path the core would refuse, answers null.
  const file = assetHref(domain, href ?? "");
  if (file !== null) {
    return (
      <a href={file} target="_blank" rel="noreferrer" className={LINK_CLASSES}>
        {children}
      </a>
    );
  }
  const outward = typeof href === "string" && /^https?:\/\//i.test(href);
  return (
    <a
      href={href}
      className={LINK_CLASSES}
      {...(outward ? { target: "_blank", rel: "noreferrer" } : {})}
    >
      {children}
    </a>
  );
}

/**
 * One image: an attachment of this domain drawn from the files route, or
 * whatever it was written as.
 *
 * Only a relative `assets/` target is rewritten. An absolute path and an
 * external URL are somebody else's address and are handed to the browser
 * exactly as the author wrote them - including any fragment, which means
 * nothing to this app out there.
 *
 * The placement fragment is read on the way through, and the style it means is
 * the same one the editor's preview widget applies, so a floated image looks
 * the same in both places.
 */
function MarkdownImage({
  src,
  alt,
  domain,
}: {
  src?: string;
  alt?: string;
  domain?: string;
}) {
  const written = typeof src === "string" ? src : "";
  const file = assetHref(domain, written);
  // The directives are read off the decoded target for the same reason the
  // path is: micromark hands `w=50%` over as `w=50%25`, which is no width at
  // all. Decoding happens once, here and in `assetPath`, never in sequence.
  const { format } = parseImageFragment(decodeTarget(written));
  return (
    <img
      // Ours is rebuilt from the decoded path, which re-encodes exactly once;
      // anything else keeps the URL the renderer produced, escapes and all.
      src={file ?? written}
      // Never null: react-markdown hands the alt text through as written, and
      // an image with no alt at all is one a screen reader cannot skip.
      alt={alt ?? ""}
      loading="lazy"
      style={imageStyle(file === null ? { align: "center" } : format)}
    />
  );
}

/**
 * The component map for one domain: everything above, plus the two elements
 * that need to know which domain a relative `assets/` target belongs to.
 */
function componentsFor(domain: string | undefined): Components {
  return {
    ...components,
    a: ({ children, href, node }) => (
      <MarkdownAnchor
        href={href}
        node={node}
        {...(domain === undefined ? {} : { domain })}
      >
        {children}
      </MarkdownAnchor>
    ),
    img: ({ src, alt }) => (
      <MarkdownImage
        {...(typeof src === "string" ? { src } : {})}
        {...(typeof alt === "string" ? { alt } : {})}
        {...(domain === undefined ? {} : { domain })}
      />
    ),
  };
}

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
  foldTitle,
  domain,
}: {
  source: string;
  wikilinks?: WikilinkResolver;
  foldTitle?: string;
  /**
   * The domain this document lives in, which is what a relative `assets/`
   * target is relative to. Absent, such a target is left as written: an
   * attachment path means nothing without the domain holding it.
   */
  domain?: string;
}) {
  const componentMap = useMemo(() => componentsFor(domain), [domain]);
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
    // `measured` here rather than on a page wrapper: the rule is
    // `.measured > :not(.breakout)`, so it has to sit on the one element
    // whose direct children are the document's own blocks. A wrapper around
    // this renderer would see a single child - this div - and cap the tables
    // and diagrams inside it along with the prose. Hardcoded rather than a
    // prop, because every markdown surface is a reading surface.
    <div className="measured text-[0.95rem]">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={rehypePlugins}
        components={componentMap}
      >
        {foldLeadingTitle(source.replace(FRONTMATTER, ""), foldTitle)}
      </ReactMarkdown>
    </div>
  );
}
