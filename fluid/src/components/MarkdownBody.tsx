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

import { Link as LinkIcon } from "lucide-react";
import {
  Children,
  createContext,
  isValidElement,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ComponentProps, ReactNode } from "react";
import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import type { Components } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import { Link, useLocation, useNavigate } from "react-router";

import {
  SECTION_FLASH_CLASS,
  SECTION_FLASH_MS,
  headingIds,
  sectionUrl,
} from "../anchors";
import type { AssetResolver } from "../api/files";
import { documentHref } from "../base";
import {
  decodeTarget,
  imageStyle,
  parseImageFragment,
} from "../editor/imageFormat";
import { CALLOUT_KINDS, REVEAL_EVENT } from "../markdown/callouts";
import type { CalloutKind, RevealDetail } from "../markdown/callouts";
import { isStatusColour } from "../markdown/directives";
import { SWATCH_CLASSES, colourOf } from "../markdown/swatch";
import { dropHtmlComments } from "../markdown/htmlComments";
import { keepWithNext } from "../markdown/keepWithNext";
import { remarkPipeline } from "../markdown/pipeline";
import { printUrl } from "../printUrl";
import { SOURCE_ATTR, spanPieces } from "../reflectionQuotes";
import { WIKILINK, referenceState, wikilinkWords } from "../wikilinks";
import type { WikilinkResolution, WikilinkResolver } from "../wikilinks";
import Callout from "./Callout";
import CopyCode from "./CopyCode";
import DiagramOverlay from "./DiagramOverlay";
import DiagramToolbar from "./DiagramToolbar";
import { imageFileName } from "./downloads";
import { lazyPart } from "./lazyPart";
import { Chip, FOCUS_RING } from "./primitives";
import StatusLabel from "./StatusLabel";
import { NotApplicableBox, TaskBox } from "./TaskBox";
import { useEngramAssetHome } from "./useEngramAssetHome";
import { useSaid } from "./useSaid";

const MermaidDiagram = lazyPart(() => import("./MermaidDiagram"));
const MathPart = lazyPart(() => import("./MathPart"));

/**
 * The renderer's own plugin-list type, taken from its props rather than from
 * the unified packages underneath it, which are a transitive dependency this
 * package does not declare.
 */
type RehypePlugins = ComponentProps<typeof ReactMarkdown>["rehypePlugins"];
type RemarkPlugins = ComponentProps<typeof ReactMarkdown>["remarkPlugins"];

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
  /** Where the parser found the node, for the source-span stamp. */
  position?: { start?: { offset?: number }; end?: { offset?: number } };
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
      // of marking a reference, and a link marks it now. The resolver already
      // made the author's display text (`[[a|b]]`) the label.
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

/**
 * Stamp every text node the parser positioned with its offset into the
 * source, for the reflection page: a selection's ends read the offsets back
 * and become a quote of the source text a comment anchors on
 * (`reflectionQuotes.ts`). First in the chain, so the positions are the
 * parser's own. Text the parser gives no position (a fenced code block) or
 * whose words are not one piece of the source (an escape, an entity, a
 * continuation line) stays unstamped, and a selection that ends there becomes
 * a section comment rather than a guessed quote.
 *
 * A section id comment is gone by then (`dropHtmlComments`).
 */
function sourceSpanStamp(source: string) {
  return function plugin() {
    return function transform(tree: unknown) {
      stamp(tree as HastNode, source);
    };
  };
}

/** Wrap each positioned text child of `node` in a stamped span, all the way down. */
function stamp(node: HastNode, source: string): void {
  if (!node.children) {
    return;
  }
  node.children = node.children.flatMap((child) => {
    if (child.type !== "text") {
      stamp(child, source);
      return [child];
    }
    const pieces = spanPieces(
      source,
      child.position?.start?.offset,
      child.position?.end?.offset,
      child.value ?? "",
    );
    if (pieces.length === 1 && pieces[0]?.base === null) {
      return [child];
    }
    return pieces.map(({ value, base }) =>
      base === null
        ? { type: "text", value }
        : {
            type: "element",
            tagName: "span",
            properties: { [SOURCE_ATTR]: String(base) },
            children: [{ type: "text", value }],
          },
    );
  });
}

/**
 * Stamp the words of a fenced code block, after the highlighter.
 *
 * The highlighter rebuilds a fence's children from its text, so a stamp
 * made before it would be lost, and the text node of a fence carries no
 * position of its own anyway. The `code` element does: its fence spans the
 * opening line, the code and the closing line. The code starts on the line
 * after the opening one; where the source holds the block's text there as
 * one piece (a fence at the margin), every text node the highlighter made
 * is stamped at its running offset, in order. A fence whose lines carry an
 * indent or a quote marker is left unstamped: a selection in it is a comment
 * on the section, and a quote in it is not painted, never painted wrong. A
 * diagram is no text on the page and is left alone.
 */
function codeSpanStamp(source: string) {
  return function plugin() {
    return function transform(tree: unknown) {
      const walk = (node: HastNode, parent: HastNode | null): void => {
        if (
          node.type === "element" &&
          node.tagName === "code" &&
          parent?.tagName === "pre"
        ) {
          stampFence(node, source);
          return;
        }
        for (const child of node.children ?? []) {
          walk(child, node);
        }
      };
      walk(tree as HastNode, null);
    };
  };
}

/** Stamp one fence's text nodes at their offsets, or leave it as it is. */
function stampFence(code: HastNode, source: string): void {
  if (languageOf(code) === "mermaid" || languageOf(code) === "math") {
    return;
  }
  const start = code.position?.start?.offset;
  const end = code.position?.end?.offset;
  const text = textOf(code);
  if (start === undefined || end === undefined || text === "") {
    return;
  }
  const opening = source.indexOf("\n", start);
  if (opening < 0 || opening >= end) {
    return;
  }
  const base = opening + 1;
  if (
    base + text.length > end ||
    source.slice(base, base + text.length) !== text
  ) {
    return;
  }
  let at = base;
  const wrap = (node: HastNode): void => {
    node.children = (node.children ?? []).map((child) => {
      if (child.type !== "text") {
        wrap(child);
        return child;
      }
      const value = child.value ?? "";
      const stamped: HastNode = {
        type: "element",
        tagName: "span",
        properties: { [SOURCE_ATTR]: String(at) },
        children: [{ type: "text", value }],
      };
      at += value.length;
      return value === "" ? child : stamped;
    });
  };
  wrap(code);
}

/**
 * The line under the first title, handed to the heading through context and
 * not through the component map or the plugin chain: those are memoised so a
 * changed line cannot remount what the document drew (images, diagrams, an
 * open full window, a selection in progress).
 */
const TitleNoteContext = createContext<string | undefined>(undefined);

/** The marker the first level-one heading carries for `titleNote`. */
const FIRST_TITLE = "dataFirstTitle";
/** The same marker as the component receives it: hast `dataFirstTitle` is `data-first-title`. */
const FIRST_TITLE_PROP = "data-first-title";

/**
 * A small control drawn at the right end of the document's first heading,
 * outside its text, handed down the same way as `titleNote` and for the same
 * reason: a changed control re-renders its heading and remounts nothing.
 */
const HeadingAsideContext = createContext<HeadingAside | undefined>(undefined);

/**
 * What a page draws beside its first heading: `aside` inside the heading at
 * its right end, `after` straight after it in a zero-height box, for
 * something laid over the text (a list the control opens) that must not be
 * part of the heading.
 */
export interface HeadingAside {
  aside: ReactNode;
  after?: ReactNode;
}

/** The marker the first heading of any level carries for `headingAside`. */
const FIRST_HEADING = "dataFirstHeading";
/** The same marker as the component receives it. */
const FIRST_HEADING_PROP = "data-first-heading";

/**
 * Mark the document's first `<h1>`, in document order, so the heading
 * component can draw a note straight under it. Walks the tree once and
 * changes nothing but that one property; first in the chain, before the
 * source-span stamp, so no position moves.
 */
function markFirstTitle() {
  return function transform(tree: unknown) {
    const walk = (node: HastNode): boolean => {
      if (node.type === "element" && node.tagName === "h1") {
        node.properties = { ...node.properties, [FIRST_TITLE]: "true" };
        return true;
      }
      return (node.children ?? []).some(walk);
    };
    walk(tree as HastNode);
    const first = (node: HastNode): boolean => {
      if (node.type === "element" && /^h[1-6]$/.test(node.tagName ?? "")) {
        node.properties = { ...node.properties, [FIRST_HEADING]: "true" };
        return true;
      }
      return (node.children ?? []).some(first);
    };
    first(tree as HastNode);
  };
}

/** The offset of a task box's `[` in `shown`, as `remarkTaskState` stamps it. */
const TASK_AT = "data-task-at";
/** The words a task box is named by, as {@link taskBoxes} copies them. */
const TASK_LABEL = "data-task-label";
/** The element {@link taskBoxes} adds where a not-applicable box is drawn. */
const TASK_NA_BOX = "data-task-box";

/**
 * An item's own words: its text without the lists nested in it, and a
 * wikilink read as its target's words (the rewrite that draws the link runs
 * later in the chain, so the brackets are still in the text here).
 */
function itemWords(node: HastNode): string {
  return words(node).replace(
    WIKILINK,
    (link, inner: string) => wikilinkWords(inner) ?? link,
  );
}

function words(node: HastNode): string {
  return (node.children ?? [])
    .map((child) =>
      child.type === "element" &&
      (child.tagName === "ul" || child.tagName === "ol")
        ? ""
        : child.type === "element" && child.tagName === "p"
          ? words(child)
          : textOf(child),
    )
    .join("")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * Hand every task box what it is drawn from: the item's words, which name
 * it, and the offset `remarkTaskState` found for its `[`. The checkbox is
 * GitHub's own `input`, which the converter puts first in the item, or first
 * in the item's paragraph when the list is loose; a not-applicable item gets
 * an element of its own in the same place. First in the chain, before the
 * stamp wraps the words in spans.
 */
function taskBoxes() {
  return function transform(tree: unknown) {
    const walk = (node: HastNode): void => {
      if (node.type === "element" && node.tagName === "li") {
        const at = node.properties?.[TASK_AT];
        const na = node.properties?.["data-task"] === "na";
        if (na || isTaskItem(node)) {
          const first = node.children?.find(
            (child) => child.type === "element",
          );
          const holder = first?.tagName === "p" ? first : node;
          const label = itemWords(node);
          if (na) {
            holder.children = [
              {
                type: "element",
                tagName: "span",
                properties: { [TASK_NA_BOX]: "na", [TASK_LABEL]: label },
                children: [],
              },
              ...(holder.children ?? []),
            ];
          } else {
            const input = (holder.children ?? []).find(
              (child) => child.type === "element" && child.tagName === "input",
            );
            if (input !== undefined) {
              input.properties = {
                ...input.properties,
                [TASK_LABEL]: label,
                ...(typeof at === "string" ? { [TASK_AT]: at } : {}),
              };
            }
          }
        }
      }
      for (const child of node.children ?? []) {
        walk(child);
      }
    };
    walk(tree as HastNode);
  };
}

/** Whether remark-gfm made this item a task item. */
function isTaskItem(node: HastNode): boolean {
  const names = node.properties?.className;
  return Array.isArray(names) && names.includes("task-list-item");
}

/**
 * What a task box may do here, handed down by context rather than through
 * the component map, for the same reason as `TitleNoteContext`: a page's
 * changing callback must not remount the document. Undefined, a box only
 * shows its state, as it always did.
 */
interface TaskTick {
  /** Flip the box whose `[` is at this offset into `shown`. Absent, nothing ticks. */
  tick?: (at: number, next: "[ ]" | "[x]") => void;
  /** Why nothing ticks, shown with every box. */
  reason?: string;
}

const TaskTickContext = createContext<TaskTick | undefined>(undefined);

/**
 * What the server made of the engram's references, which is where a relative
 * note link (`../pumps/p-12.md`) learns the page it opens. Only an engram page
 * has one. A context rather than a prop of the component map, because the
 * resolver changes when the graph lands, and a new component map would draw
 * the whole document again.
 */
const NoteLinksContext = createContext<WikilinkResolver | undefined>(undefined);

/** One GitHub task box, wired to what the page allows. */
function TaskInput({
  checked,
  at,
  label,
}: {
  checked: boolean;
  at: number | null;
  label: string;
}) {
  const task = useContext(TaskTickContext);
  const tick = task?.tick;
  return (
    <TaskBox
      checked={checked}
      label={label}
      reason={task?.reason}
      onToggle={
        tick === undefined || at === null
          ? undefined
          : () => {
              tick(at, checked ? "[ ]" : "[x]");
            }
      }
    />
  );
}

/** The source offset a stamped span carries, or undefined for any other span. */
function sourceStamp(node: unknown): string | undefined {
  const value = (node as HastNode | undefined)?.properties?.[SOURCE_ATTR];
  return typeof value === "string" ? value : undefined;
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

/** An inline `#tag`, as the observation grammar reads one. */
const INLINE_TAG = /(^|\s)#([A-Za-z][A-Za-z0-9_-]*)/g;

/**
 * A string with its inline `#tags` drawn as accent chips, the way the
 * mockup shows an observation's tags. Text without a tag comes back as it was.
 */
function withTagChips(text: string, keyBase: string): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(INLINE_TAG)) {
    const lead = match[1] ?? "";
    const start = match.index + lead.length;
    out.push(text.slice(last, start));
    out.push(
      <Chip variant="accent" key={`${keyBase}:${String(start)}`}>
        #{match[2]}
      </Chip>,
    );
    last = match.index + match[0].length;
  }
  if (last === 0) {
    return [text];
  }
  out.push(text.slice(last));
  return out;
}

/** A marked line: the mark as a chip, then the rest of the line as written. */
function chipped(mark: string, rest: string, parts: ReactNode[]): ReactNode {
  return [
    <Chip mono key="mark">
      {mark}
    </Chip>,
    " ",
    ...withTagChips(rest, "rest"),
    ...parts
      .slice(1)
      .map((part, index) =>
        typeof part === "string"
          ? withTagChips(part, `p${String(index)}`)
          : part,
      ),
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

/** The callout a component's props describe, or the element as it came. */
function calloutOf(
  props: Record<string, unknown>,
  children: ReactNode,
  tag: "aside" | "details",
): ReactNode {
  const kind = props["data-callout"];
  if (
    typeof kind !== "string" ||
    !(CALLOUT_KINDS as readonly string[]).includes(kind)
  ) {
    const Tag = tag;
    return <Tag>{children}</Tag>;
  }
  const title = props["data-title"];
  const fold = props["data-fold"];
  return (
    <Callout
      kind={kind as CalloutKind}
      title={typeof title === "string" ? title : null}
      fold={fold === "open" || fold === "closed" ? fold : null}
    >
      {children}
    </Callout>
  );
}

/**
 * How each element is drawn.
 *
 * Written out rather than pulled from a typography plugin: the app owns a
 * handful of colors and one dark variant, and a plugin would bring a second
 * opinion about all of them.
 *
 * The six heading levels are not here: they are built per document by the
 * factory below, because whether a heading carries a link symbol is a fact
 * about the surface rather than about the tag.
 */
const components: Components = {
  p: ({ children }) => <p className="my-3 leading-relaxed">{children}</p>,
  ul: ({ children }) => (
    <ul className="my-3 list-disc pl-6 leading-relaxed">{children}</ul>
  ),
  ol: ({ children }) => (
    <ol className="my-3 list-decimal pl-6 leading-relaxed">{children}</ol>
  ),
  // The id is what a footnote reference jumps to. A task item draws its
  // box where the bullet would be, so it has no bullet of its own.
  // `data-short` from `keepWithNext`: an item the print sheet keeps whole.
  li: ({ children, id, className, node, ...props }) => (
    <li
      id={id}
      data-short={node?.properties["dataShort"] === undefined ? undefined : ""}
      className={
        className?.includes("task-list-item") === true ||
        (props as Record<string, unknown>)["data-task"] === "na"
          ? "my-1 list-none"
          : "my-1"
      }
    >
      {structuredBullet(children)}
    </li>
  ),
  // Only GitHub's task boxes make an input here: raw HTML stays text.
  input: ({ checked, type, ...props }) => {
    if (type !== "checkbox") {
      return null;
    }
    const extra = props as Record<string, unknown>;
    const at = extra[TASK_AT];
    const label = extra[TASK_LABEL];
    return (
      <TaskInput
        checked={checked === true}
        at={typeof at === "string" ? Number(at) : null}
        label={typeof label === "string" ? label : ""}
      />
    );
  },
  span: ({ children, className, node, ...props }) => {
    // Four sources make spans in this tree, all plugin-generated rather than
    // raw HTML an engram could write: the wikilink rewrite's own markers,
    // rehype-highlight's per-token spans inside a fenced code block (the
    // `.hljs-string` etc. classes `index.css`'s syntax palette styles), the
    // source-span stamp, and a status label from the directives plugin. A
    // status label and the wikilink case get their own fixed look;
    // everything else has to keep whatever class it arrived with; passing an
    // incoming `className` through unconditionally, rather than dropping it,
    // is what makes a code fence render in color instead of the page's plain
    // text color. A stamp keeps its offset, and a span without one renders
    // exactly as before, because an undefined attribute is never written.
    const status = (props as Record<string, unknown>)["data-status"];
    if ((props as Record<string, unknown>)[TASK_NA_BOX] === "na") {
      const label = (props as Record<string, unknown>)[TASK_LABEL];
      return (
        <NotApplicableBox label={typeof label === "string" ? label : ""} />
      );
    }
    if (isStatusColour(status)) {
      return <StatusLabel colour={status}>{children}</StatusLabel>;
    }
    return wikilinkKind(node) === "unresolved" ? (
      <span
        title="not resolved"
        className="underline decoration-dotted underline-offset-2 opacity-70"
      >
        {children}
      </span>
    ) : (
      <span className={className} data-src={sourceStamp(node)}>
        {children}
      </span>
    );
  },
  blockquote: ({ children }) => (
    <blockquote className="my-3 border-l-2 border-slate-300 pl-4 text-slate-600 dark:border-slate-700 dark:text-slate-300">
      {children}
    </blockquote>
  ),
  // A callout, named by `remarkCallouts`; nothing else in this tree makes an
  // `aside` or a `details`, as raw HTML stays text.
  aside: ({ children, ...props }) => calloutOf(props, children, "aside"),
  details: ({ children, ...props }) => calloutOf(props, children, "details"),
  // The reference mark: a small raised accent link, as `a` already draws it.
  sup: ({ children }) => (
    <sup className="text-[0.75em] leading-none [&_a]:no-underline">
      {children}
    </sup>
  ),
  // The notes at the end: a hairline above, meta size, the label in the
  // section-label style (see `heading`).
  // The converter's notes section is the only `section` a text can hold
  // (raw HTML stays literal); its hast node says so with `dataFootnotes`.
  section: ({ children, node }) =>
    node?.properties["dataFootnotes"] !== true ? (
      <section>{children}</section>
    ) : (
      <section
        data-footnotes=""
        className="footnotes mt-10 border-t border-hair pt-3 text-[13px] text-ink-3 [&_ol]:my-1 [&_p]:my-1"
      >
        {children}
      </section>
    ),
  hr: () => <hr className="my-6 border-slate-200 dark:border-slate-800" />,
  table: ({ children, node }) => (
    // Wide tables scroll inside themselves rather than widening the page, and
    // keep the full column rather than the reading measure.
    <div className="breakout my-4 overflow-x-auto">
      <table
        // Short enough to keep on one page in print (`keepWithNext`).
        data-short={
          node?.properties["dataShort"] === undefined ? undefined : ""
        }
        className="w-full border-collapse text-sm"
      >
        {children}
      </table>
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
  code: ({ children, className, node }) => {
    // A fenced block carries a language class or the highlighter's own, and
    // sits inside a `pre` that is already the box. Only the inline kind gets
    // drawn as a chip.
    if (typeof className === "string" && className.includes("math-inline")) {
      const tex = textOf(node as HastNode | undefined);
      return (
        <MathPart fallback={<code>{tex}</code>} tex={tex} display={false} />
      );
    }
    const fenced =
      typeof className === "string" &&
      (className.includes("language-") || className.includes("hljs"));
    if (fenced) {
      return <code className={className}>{children}</code>;
    }
    // A colour code alone in the span gets a swatch of that colour before it.
    // Read from the node, as a stamped span wraps the text in its own span.
    const colour = colourOf(textOf(node as HastNode | undefined));
    const chip = (
      <code className="rounded bg-slate-100 px-1 py-0.5 font-mono text-[0.9em] dark:bg-slate-800">
        {children}
      </code>
    );
    return colour === null ? (
      chip
    ) : (
      <>
        <span
          data-swatch
          aria-hidden="true"
          style={{ background: colour }}
          className={SWATCH_CLASSES}
        />
        {chip}
      </>
    );
  },
};

/**
 * The fenced block, which is a diagram wherever the fence says mermaid.
 *
 * It takes the document's name rather than reading it from anywhere, because
 * a diagram a reader takes out of the full window is named after the document
 * it came from and only the caller knows which document that is.
 */
function preFor(
  documentName: string | undefined,
): NonNullable<Components["pre"]> {
  return ({ children, node }) => {
    const fence = (node as HastNode | undefined)?.children?.[0];
    const source = textOf(fence);
    if (languageOf(fence) === "math" && source.trim() !== "") {
      return (
        <div className="breakout my-4 overflow-x-auto">
          <MathPart
            fallback={<code>{source}</code>}
            tex={source}
            display={true}
          />
        </div>
      );
    }
    if (languageOf(fence) === "mermaid" && source.trim() !== "") {
      return (
        <figure
          aria-label="Diagram"
          className="breakout my-4 rounded border border-slate-200 p-3 dark:border-slate-800"
        >
          <MermaidDiagram
            fallback={<DiagramSource source={source} />}
            source={source}
            {...(documentName === undefined ? {} : { name: documentName })}
          />
        </figure>
      );
    }
    return (
      <div className="group breakout relative my-4">
        <pre className="overflow-x-auto rounded bg-slate-100 p-3 text-sm dark:bg-slate-900">
          {children}
        </pre>
        <CopyCode text={source.replace(/\n$/, "")} />
      </div>
    );
  };
}

/**
 * The URL a target's bytes are read at, or null when it names no stored file
 * - which includes every case where the caller has no resolver to ask.
 *
 * The single place the reading view turns a written target into an address,
 * asked by the image and the anchor alike, and it asks the resolver the same
 * question the rail asks (for an engram, `assetPath`) so the two can never
 * disagree about a file.
 */
function assetHref(
  assets: AssetResolver | undefined,
  target: string,
): string | null {
  if (assets === undefined) {
    return null;
  }
  const path = assets.resolve(target);
  return path === null ? null : assets.url(path);
}

/** Why the resolver will not have this target fetched, or null. */
function refusalOf(
  assets: AssetResolver | undefined,
  target: string,
): string | null {
  return assets === undefined ? null : assets.refused(target);
}

/**
 * The page a relative link to another engram's file opens, or null when it is
 * not one or the server did not resolve it. The resolver is asked with the
 * path the way the server parsed it, decoded and without the fragment; the
 * fragment goes on the route, so `#heading` still lands on the heading.
 */
function noteRoute(
  notes: WikilinkResolver | undefined,
  href: string | undefined,
): string | null {
  if (notes === undefined || href === undefined) {
    return null;
  }
  const hash = href.indexOf("#");
  const written = hash === -1 ? href : href.slice(0, hash);
  const fragment = hash === -1 ? "" : href.slice(hash);
  if (
    !/\.md$/i.test(written) ||
    written.startsWith("/") ||
    /^[a-z][a-z0-9+.-]*:/i.test(written)
  ) {
    return null;
  }
  let path = written;
  try {
    path = decodeURIComponent(written);
  } catch {
    // A target that is not encoded is read as it is.
  }
  const resolution = notes(path);
  return resolution?.kind === "resolved"
    ? `${resolution.href}${fragment}`
    : null;
}

/** The classes every link in a document wears, wherever it points. */
const LINK_CLASSES =
  "text-link underline decoration-(--link-line) decoration-1 underline-offset-[3px] hover:no-underline";

/**
 * One link, drawn by where it points.
 *
 * Three destinations, in the order they are decided. A resolved wikilink is a
 * screen of this app and navigates in place; the route was built by the
 * resolver from the graph's own addresses, so there is nothing left to
 * validate. A relative link to another engram's file (`../pumps/p-12.md`) on an
 * engram page is one too, when the server resolved it: its path is not the
 * page's address, so the route is the one the server's answer gives. An
 * `assets/` target is an attachment of this engram's own domain
 * and points at the files route, in a new tab because what comes back is a pdf
 * or a spreadsheet rather than a page of this app - and whether that opens or
 * downloads is the server's disposition to decide, not this anchor's.
 * Everything else is what it was written as.
 *
 * Before any of that but the wikilink, the resolver may refuse a target: a
 * file this document must not read from (a reflection pointing at an engram
 * attachment). Then the words stay and the address goes, with the reason on
 * hover, so nothing is fetched and nothing navigates.
 *
 * react-markdown's own URL transform has already dropped the protocols a link
 * may not carry (`javascript:` among them). The one exception is a refused
 * `crystalline://` target, which can arrive verbatim and is never put in an
 * href. What is left to decide is where it
 * opens: a link out of the app leaves this tab alone and gets no handle on it,
 * while an anchor or an in-app path navigates in place.
 */
function MarkdownAnchor({
  children,
  href,
  node,
  assets,
  id,
}: {
  children?: ReactNode;
  /** A footnote reference carries the id its note links back to. */
  id?: string | undefined;
  /** Undefined is spelled out: react-markdown hands a link with no target on. */
  href?: string | undefined;
  node?: unknown;
  /** What a relative target resolves against, when known. */
  assets?: AssetResolver;
}) {
  const notes = useContext(NoteLinksContext);
  if (wikilinkKind(node) === "resolved" && typeof href === "string") {
    return (
      <Link to={href} className={LINK_CLASSES}>
        {children}
      </Link>
    );
  }
  const note = noteRoute(notes, href);
  if (note !== null) {
    return (
      <Link to={note} className={LINK_CLASSES}>
        {children}
      </Link>
    );
  }
  const refusal = refusalOf(assets, href ?? "");
  if (refusal !== null) {
    return <span title={refusal}>{children}</span>;
  }
  // One question decides it, and the rail asks the same one: does this target
  // name a file of this domain? A `./` prefix is stripped, the fragment is
  // dropped - the files route never sees one - and an address that is somebody
  // else's, or a path the core would refuse, answers null.
  const file = assetHref(assets, href ?? "");
  if (file !== null) {
    return (
      <a href={file} target="_blank" rel="noreferrer" className={LINK_CLASSES}>
        {children}
      </a>
    );
  }
  const outward = typeof href === "string" && /^https?:\/\//i.test(href);
  // Resolved against the page it is on rather than against the base tag, so
  // `#heading` and `../other` go where the author meant (`documentHref`).
  return (
    <a
      id={id}
      href={documentHref(href)}
      className={LINK_CLASSES}
      {...(outward ? { target: "_blank", rel: "noreferrer" } : {})}
      data-print-url={
        typeof href === "string"
          ? (printUrl(href, textOf(node as HastNode | undefined)) ?? undefined)
          : undefined
      }
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
 *
 * In the column an image is as wide as the prose lets it be, which for a
 * screenshot of anything is too small to read, so it opens in the same
 * full-window layer a diagram does - by the button in its corner, or by a
 * click on the picture, which is what a reader tries first. A `span` and not
 * a `figure`, because the wrapper stands where the image stood and an image
 * lives inside a paragraph, where only phrasing content may go.
 *
 * A target the resolver refuses is not drawn at all, so its bytes are never
 * asked for: the alt text stands where the picture would, with the reason
 * under it.
 */
function MarkdownImage({
  src,
  alt,
  assets,
}: {
  src?: string;
  alt?: string;
  assets?: AssetResolver;
}) {
  const written = typeof src === "string" ? src : "";
  const refusal = refusalOf(assets, written);
  const file = assetHref(assets, written);
  // The directives are read off the decoded target for the same reason the
  // path is: micromark hands `w=50%` over as `w=50%25`, which is no width at
  // all. Decoding happens once, here and in `assetPath`, never in sequence.
  const decoded = decodeTarget(written);
  const { format } = parseImageFragment(decoded);
  const wrapperRef = useRef<HTMLSpanElement>(null);
  const fullWindowRef = useRef<HTMLButtonElement>(null);
  const [fullWindow, setFullWindow] = useState(false);
  // Whether a link is in charge here, which only the tree can answer: this
  // component is handed its own props and nothing about what it sits in, so
  // the answer comes from the DOM once it is in one. An image inside a link
  // belongs to the link, and a corner button offering a second destination
  // beside it is a choice nobody asked for.
  const [linked, setLinked] = useState(false);
  useEffect(() => {
    const wrapper = wrapperRef.current;
    setLinked(wrapper !== null && wrapper.closest("a") !== null);
  }, []);
  if (refusal !== null) {
    return (
      <span>
        {alt}
        <span className="block text-caption text-slate-500 dark:text-slate-400">
          {refusal}
        </span>
      </span>
    );
  }
  // The URL transform dropped the target (an unknown protocol, say): there is
  // nothing to fetch, so the alt text stands alone, with no picture and no
  // full-window button.
  if (written === "") {
    return <span>{alt}</span>;
  }
  // Ours is rebuilt from the decoded path, which re-encodes exactly once;
  // anything else keeps the URL the renderer produced, escapes and all.
  const shown = file ?? written;
  const placement = imageStyle(file === null ? { align: "center" } : format);
  // The wrapper takes the placement the image had - the float, the margins,
  // the block and any width the fragment asked for - and hugs the picture
  // where no width was asked for, so the toolbar's corner is the image's
  // corner rather than the column's. The image fills a wrapper that was given
  // a width and keeps its own size otherwise, where a share of the column
  // would be a share of itself.
  const wrapperStyle = {
    ...placement,
    width: placement.width ?? "fit-content",
    maxWidth: "100%",
  };
  const fills = placement.width !== undefined;
  return (
    <span ref={wrapperRef} className="group relative" style={wrapperStyle}>
      <img
        src={shown}
        // Never null: react-markdown hands the alt text through as written,
        // and an image with no alt at all is one a screen reader cannot skip.
        alt={alt ?? ""}
        loading="lazy"
        className={`h-auto max-w-full${fills ? " w-full" : ""}${
          linked ? "" : " cursor-zoom-in"
        }`}
        onClick={(event) => {
          // Inside a link the link wins: the click is the reader's way to
          // follow it, and the full window stays a hover away on the button -
          // except that inside a link there is no button either.
          if (event.currentTarget.closest("a") !== null) {
            return;
          }
          setFullWindow(true);
        }}
      />
      {!linked && (
        <DiagramToolbar
          onOpenFullWindow={() => {
            setFullWindow(true);
          }}
          fullWindowRef={fullWindowRef}
        />
      )}
      {fullWindow && (
        <DiagramOverlay
          content={{
            kind: "image",
            src: shown,
            alt: alt ?? "",
            // The name the author wrote, not the address this app built: a
            // file saved out of the full window keeps the name it has in the
            // document rather than a route's last segment. The target goes
            // over as written, because the helper decodes it once itself.
            filename: imageFileName(written),
          }}
          returnFocusTo={fullWindowRef}
          onClose={() => {
            setFullWindow(false);
          }}
        />
      )}
    </span>
  );
}

/**
 * What a section URL in this document is built from.
 *
 * One page URL, because a section is the page plus a fragment. Absent, the
 * document has no anchors at all: no ids on its headings, no link symbol
 * beside them and no reaction to a fragment in the location. A surface that is
 * not addressable should not pretend to be.
 */
export interface MarkdownAnchors {
  /** The page's own URL, as the server spelled it for this reader. */
  pageUrl: string;
}

/**
 * What a link symbol needs to know, which is more than the component map can
 * carry.
 *
 * It rides a context rather than the component map on purpose. The map decides
 * which element each tag becomes, and react-markdown remounts a heading when
 * the component behind its tag changes identity - so a map rebuilt every time
 * somebody copies a link would take the focus off the very button they just
 * pressed. The map therefore knows one unchanging thing (whether this document
 * has anchors at all) and everything that moves arrives here.
 */
interface SectionAnchorState {
  pageUrl: string;
  /** What the document is saying right now, or null for silence. */
  said: string | null;
  /** Which heading said it, so the text is drawn beside that one. */
  activeId: string | null;
  activate: (id: string) => void;
}

const SectionAnchors = createContext<SectionAnchorState | null>(null);

/**
 * The state behind every link symbol in one document.
 *
 * A component of its own, wrapped around the rendered document rather than
 * holding it: the document arrives as `children`, so saying something
 * re-renders this component and the announcement, and leaves the whole parsed
 * document exactly where it was.
 */
function SectionAnchorProvider({
  pageUrl,
  children,
}: {
  pageUrl: string;
  children: ReactNode;
}) {
  const [said, say] = useSaid();
  const [activeId, setActiveId] = useState<string | null>(null);
  const navigate = useNavigate();
  const { search } = useLocation();
  const activate = useCallback(
    (id: string) => {
      setActiveId(id);
      // Replace rather than push: a reader pointing at sections of the page
      // they are already on is not walking a history, and a Back that stepped
      // through every heading they glanced at would be useless. The hash
      // landing in the location is what runs the arrival behaviour, so the
      // highlight on a click is the very same highlight as on arrival.
      //
      // The query string is carried along rather than left out: a partial
      // destination REPLACES the parts it does not name, so naming only the
      // hash would drop whatever the page was opened with.
      void navigate({ search, hash: `#${id}` }, { replace: true });
      void (async () => {
        try {
          // `navigator.clipboard` is absent on an insecure or older context
          // and reading `.writeText` off it throws synchronously, which is
          // why this is a try/catch rather than a `.catch` off the promise.
          await navigator.clipboard.writeText(sectionUrl(pageUrl, id));
          say("Link copied");
        } catch {
          say("Copy refused");
        }
      })();
    },
    [navigate, pageUrl, say, search],
  );
  const state = useMemo(
    () => ({ pageUrl, said, activeId, activate }),
    [pageUrl, said, activeId, activate],
  );
  return (
    <SectionAnchors.Provider value={state}>{children}</SectionAnchors.Provider>
  );
}

/**
 * The link symbol beside one heading.
 *
 * Quiet until the heading is hovered or the button itself has keyboard focus,
 * but never `aria-hidden` and never out of the tab order: a control that hides
 * from a pointer is a courtesy, one that hides from a keyboard or a screen
 * reader is a control that is not there. Its name says what it does rather
 * than naming the icon.
 */
function SectionLink({ id }: { id: string }) {
  const anchors = useContext(SectionAnchors);
  if (!anchors) {
    return null;
  }
  return (
    <>
      <button
        type="button"
        aria-label="Link to this section"
        className={`ml-2 inline-flex align-middle opacity-0 group-hover:opacity-100 focus-visible:opacity-100 print:hidden ${FOCUS_RING}`}
        onClick={() => {
          anchors.activate(id);
        }}
      >
        <LinkIcon size={14} />
      </button>
      {anchors.said !== null && anchors.activeId === id && (
        <span className="ml-1 text-caption text-slate-500 print:hidden dark:text-slate-400">
          {anchors.said}
        </span>
      )}
    </>
  );
}

/**
 * The one live region for the whole document.
 *
 * In the document from the start and empty, so the text arriving in it is what
 * gets read out; one of them rather than one per heading, because a document
 * with forty headings would otherwise carry forty regions announcing nothing.
 */
function SectionLinkStatus() {
  const anchors = useContext(SectionAnchors);
  return (
    <span
      role="status"
      aria-live="polite"
      aria-label="Section link result"
      className="text-caption text-slate-500 print:hidden dark:text-slate-400"
    >
      {anchors?.said ?? ""}
    </span>
  );
}

/**
 * One heading level, drawn.
 *
 * All six are built here, including the two that had no component before:
 * ids land on every heading either way, but a link symbol needs somewhere to
 * be drawn. The fifth and sixth level are given no size, weight or margin of
 * their own, which is exactly what the framework's own reset leaves them at,
 * so they read as they always have.
 *
 * `group` and the scroll margin are on every level: the margin clears the
 * sticky top bar when a section is scrolled to, and the group is what lets the
 * link symbol appear on hover without a hover state of its own.
 */
function heading(
  level: 1 | 2 | 3 | 4 | 5 | 6,
  sizes: string,
  anchored: boolean,
) {
  const Tag = `h${level}` as const;
  return function Heading({
    children,
    id,
    className: given,
    ...rest
  }: {
    children?: ReactNode | undefined;
    id?: string | undefined;
    className?: string | undefined;
    [FIRST_TITLE_PROP]?: string | undefined;
    [FIRST_HEADING_PROP]?: string | undefined;
  }) {
    const className = `group scroll-mt-16 ${sizes}`.trimEnd();
    // Generated rather than derived from the slug: a heading literally called
    // "Auth text" would collide with a `${slug}-text` spelling, and two
    // elements sharing an id is a reference pointing at whichever came first.
    const textId = useId();
    const titleNote = useContext(TitleNoteContext);
    // The note belongs to the one heading marked first, and is not text of the
    // document: no stamp, nothing to select (`data-anchor-skip` is what the
    // quote code passes over).
    const note =
      titleNote !== undefined && rest[FIRST_TITLE_PROP] === "true" ? (
        <p
          data-anchor-skip=""
          className="-mt-1 mb-4 text-sm text-ink-3 select-none"
        >
          {titleNote}
        </p>
      ) : null;
    // Marked in the page too, so the print sheet can fold a title the
    // printed title block already carries.
    const firstTitle = rest[FIRST_TITLE_PROP] === "true" ? "" : undefined;
    const aside = useContext(HeadingAsideContext);
    // The notes section's own label, which the converter marks screen-reader
    // only: shown here in the section-label style, with no link symbol. Told
    // apart by that class, which no heading in the text can carry (raw HTML
    // stays literal), so a heading the writer called "Footnote label" stays
    // a heading.
    if (level === 2 && given?.split(/\s+/).includes("sr-only") === true) {
      return (
        <h2 className="mb-1 text-caption font-semibold tracking-wide text-ink-3 uppercase">
          {children}
        </h2>
      );
    }
    // The control is no part of the heading's words either: unstamped,
    // unselectable, passed over by the anchoring, and left out of the
    // heading's name by naming it from the span around its text.
    const side =
      aside !== undefined && rest[FIRST_HEADING_PROP] === "true" ? (
        <span
          data-anchor-skip=""
          className="ml-auto shrink-0 self-center text-sm font-normal tracking-normal select-none print:hidden"
        >
          {aside.aside}
        </span>
      ) : null;
    if (side !== null) {
      return (
        <>
          <Tag
            id={id}
            className={`${className} flex items-baseline gap-3`}
            aria-labelledby={textId}
            data-first-title={firstTitle}
          >
            <span id={textId} className="min-w-0">
              {children}
            </span>
            {anchored && id !== undefined && <SectionLink id={id} />}
            {side}
          </Tag>
          {aside?.after !== undefined && (
            <div
              data-anchor-skip=""
              className="relative h-0 select-none print:hidden"
            >
              {aside.after}
            </div>
          )}
          {note}
        </>
      );
    }
    if (!anchored || id === undefined) {
      return (
        <>
          <Tag id={id} className={className} data-first-title={firstTitle}>
            {children}
          </Tag>
          {note}
        </>
      );
    }
    // The link symbol is a real button inside the heading, so without this
    // the heading's own name would end in "Link to this section" - in the
    // document outline and in every entry of a screen reader's heading
    // list. Naming the heading by the span around its text leaves the
    // button its own name and the tab order untouched.
    return (
      <>
        <Tag
          id={id}
          className={className}
          aria-labelledby={textId}
          data-first-title={firstTitle}
        >
          <span id={textId}>{children}</span>
          <SectionLink id={id} />
        </Tag>
        {note}
      </>
    );
  };
}

/**
 * What each level is drawn at, beside the two rules every level carries.
 *
 * The last two are empty on purpose: the fifth and sixth level were never in
 * the component map and so were never styled, and the reset leaves them at the
 * body's own size, weight and spacing. Nothing about them moves here.
 */
const HEADING_SIZES = [
  "mt-6 mb-3 text-display [&:first-child:not([data-keep]>*)]:mt-0 [.measured>[data-keep]:first-child>&:first-child]:mt-0",
  "mt-[38px] mb-3 text-prose-h2 [&:first-child:not([data-keep]>*)]:mt-0 [.measured>[data-keep]:first-child>&:first-child]:mt-0",
  "mt-6 mb-2 text-prose-h3 [&:first-child:not([data-keep]>*)]:mt-0 [.measured>[data-keep]:first-child>&:first-child]:mt-0",
  "mt-4 mb-2 font-semibold [&:first-child:not([data-keep]>*)]:mt-0 [.measured>[data-keep]:first-child>&:first-child]:mt-0",
  "",
  "",
] as const;

/**
 * The component map for one document: everything above, plus the three
 * elements that need to know more than the markdown says - what a relative
 * target resolves against, and which document a picture taken out of the
 * full window is named after.
 */
function componentsFor(
  assets: AssetResolver | undefined,
  documentName: string | undefined,
  anchored: boolean,
): Components {
  return {
    ...components,
    h1: heading(1, HEADING_SIZES[0], anchored),
    h2: heading(2, HEADING_SIZES[1], anchored),
    h3: heading(3, HEADING_SIZES[2], anchored),
    h4: heading(4, HEADING_SIZES[3], anchored),
    h5: heading(5, HEADING_SIZES[4], anchored),
    h6: heading(6, HEADING_SIZES[5], anchored),
    pre: preFor(documentName),
    a: ({ children, href, node, id }) => (
      <MarkdownAnchor
        href={href}
        id={id}
        node={node}
        {...(assets === undefined ? {} : { assets })}
      >
        {children}
      </MarkdownAnchor>
    ),
    img: ({ src, alt }) => (
      <MarkdownImage
        {...(typeof src === "string" ? { src } : {})}
        {...(typeof alt === "string" ? { alt } : {})}
        {...(assets === undefined ? {} : { assets })}
      />
    ),
  };
}

/** Open every folded callout around `target`, innermost first. */
function revealIn(root: HTMLElement | null, target: Element | null): void {
  if (root === null || target === null || !root.contains(target)) {
    return;
  }
  for (
    let fold = target.closest("details");
    fold !== null && root.contains(fold);
    fold = fold.parentElement?.closest("details") ?? null
  ) {
    fold.open = true;
  }
}

/**
 * Open the folds around the element a location fragment (`#id`) names.
 * True when that element is in `root`.
 */
function revealFragment(root: HTMLElement | null, fragment: string): boolean {
  if (root === null || fragment.length < 2) {
    return false;
  }
  let id = fragment.slice(1);
  try {
    id = decodeURIComponent(id);
  } catch {
    // A hash that is not encoded stays as it is.
  }
  const target = document.getElementById(id);
  if (target === null || !root.contains(target)) {
    return false;
  }
  revealIn(root, target);
  return true;
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
  assets,
  documentName,
  anchors,
  sourceSpans,
  titleNote,
  headingAside,
  onTick,
  canTick,
}: {
  source: string;
  /**
   * Flip one task box: `offset` is the `[` of its `[ ]` or `[x]` in
   * `source` (the whole file, frontmatter and folded title counted), `next`
   * what it becomes. Absent, a box only shows its state.
   */
  onTick?: (offset: number, next: "[ ]" | "[x]") => void;
  /**
   * Whether the boxes tick: true, or a sentence saying why not, shown with
   * every box. Absent or false, they do not tick and say nothing.
   */
  canTick?: boolean | string;
  /**
   * A small control drawn at the right end of the document's first heading
   * and counted as no part of it (see `HeadingAsideContext`).
   */
  headingAside?: HeadingAside;
  /**
   * One line of text drawn straight under the document's first level-one
   * heading and counted as no part of it: unstamped, unselectable, passed
   * over by the comment anchoring. Absent, nothing is drawn.
   */
  titleNote?: string;
  wikilinks?: WikilinkResolver;
  foldTitle?: string;
  /**
   * The domain this document lives in, which is what a relative `assets/`
   * target is relative to. Absent, such a target is left as written: an
   * attachment path means nothing without the domain holding it.
   */
  domain?: string;
  /**
   * What a relative target resolves against and where its bytes are read.
   * It wins over {@link domain}, which stands for the domain's engram files.
   */
  assets?: AssetResolver;
  /**
   * What this document is called, for the name on a file a reader takes out
   * of it. Absent, a diagram's download is named after the drawing alone.
   */
  documentName?: string;
  /**
   * The page this document is read at, which makes its headings addressable:
   * each one gets an id, a link symbol that copies the page URL plus that id,
   * and a fragment in the location scrolls to the heading it names. Absent,
   * the document is drawn exactly as it always was.
   */
  anchors?: MarkdownAnchors;
  /**
   * Stamp positioned text with its source offset, for comments on selected
   * text. Only the reflection page sets it.
   */
  sourceSpans?: boolean;
}) {
  const anchored = anchors !== undefined;
  // Footnote ids are unique per document on the page: a reflection page draws
  // one document per section, and each may use `[^1]`.
  const noteScope = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  // What the parser is handed, and so what every stamped offset counts into.
  const shown = foldLeadingTitle(source.replace(FRONTMATTER, ""), foldTitle);
  // Both cuts above take a prefix and nothing else (each is anchored at the
  // start), so an offset into `shown` is this much short of one into `source`.
  const dropped = source.length - shown.length;
  const taskTick = useMemo<TaskTick | undefined>(() => {
    if (onTick === undefined) {
      return undefined;
    }
    if (canTick === true) {
      return {
        tick: (at, next) => {
          onTick(at + dropped, next);
        },
      };
    }
    return typeof canTick === "string" ? { reason: canTick } : {};
  }, [onTick, canTick, dropped]);
  const rootRef = useRef<HTMLDivElement>(null);
  const { hash, pathname } = useLocation();
  // Memoized on its inputs so the component map below is too: a new map
  // every render would remount every image and close an open full window.
  // The domain's own files, each addressed by its current bytes: a listing
  // that reports new bytes for a file redraws its picture (a given `assets`
  // resolver brings its own and asks nothing here).
  const engramHome = useEngramAssetHome(
    assets === undefined ? domain : undefined,
    source,
  );
  const resolver = useMemo(() => assets ?? engramHome, [assets, engramHome]);
  // react-markdown drops a `crystalline:` address as an unknown protocol, and
  // with it the target the resolver would refuse. Keep exactly those, and only
  // when the address really is `crystalline://`: the refusal test reads a
  // target by its shape, so `javascript:...//../../assets/x.png` counts as a
  // refused attachment, and letting it through raw would make the sanitising
  // boundary depend on how the renderers treat a refusal. Everything else,
  // such a string included, goes through the default transform.
  const urlTransform = useMemo(
    () => (url: string) =>
      url.startsWith("crystalline://") && resolver?.refused(url) != null
        ? url
        : defaultUrlTransform(url),
    [resolver],
  );
  const componentMap = useMemo(
    () => componentsFor(resolver, documentName, anchored),
    [resolver, documentName, anchored],
  );
  // The markdown extensions, in the order they run: the shared syntax
  // (`markdown/syntax.ts`: GFM, status labels, math), then the transforms.
  const remarkPlugins = useMemo<RemarkPlugins>(
    () => remarkPipeline(shown),
    [shown],
  );
  const rehypePlugins = useMemo<RehypePlugins>(
    () => [
      // HTML comments (a section's id line, a `<!-- sources: ... -->` note)
      // are for whoever edits the file: drawn as text they would show on
      // the page. Dropping a node moves no position the next steps read.
      dropHtmlComments,
      // First of the rest, so the positions it reads are the parser's own.
      // Always, so the plugin chain never depends on the note's text: a
      // changing note re-renders its own line and remounts nothing.
      markFirstTitle,
      // Before the stamp, so it reads the item's words as plain text.
      taskBoxes,
      ...(sourceSpans ? [sourceSpanStamp(shown)] : []),
      // `plainText` keeps the highlighter's hands off a mermaid fence, whose
      // text this file reads back out to draw the diagram from.
      [rehypeHighlight, { plainText: ["mermaid", "math"] }],
      // After the highlighter, which rebuilds a fence's children.
      ...(sourceSpans ? [codeSpanStamp(shown)] : []),
      // After the highlighter, so the headings it may have rewritten are
      // already in place, and before the wikilink rewrite, so a heading's id
      // is taken from the text as written rather than from whatever a
      // resolved `[[Target]]` turned into.
      ...(anchored ? [headingIds()] : []),
      // After the highlighter, so what it built is already in place. The
      // rewrite skips code either way. With `sourceSpans` it would rewrite
      // words inside a stamped span (a resolved target becomes its label),
      // which no longer reads the source at its offset: the reflection page
      // passes no resolver, and `selectionAnchor` and `domRange` refuse such
      // a span rather than map through it (`reflectionQuotes.ts`).
      ...(wikilinks ? [wikilinkRewrite(wikilinks)] : []),
      // Last: a heading and the block after it in one wrapper that may not
      // break on paper and draws no box on screen. After everything that
      // reads a heading or its neighbours.
      keepWithNext,
    ],
    [anchored, wikilinks, sourceSpans, shown],
  );
  // A folded callout opens when what a reader is sent to is inside it: the
  // fragment in the location (the router's, and the window's for a hash set
  // from outside the router), or an element another part of the page asks to
  // show (`REVEAL_EVENT`, from the threads beside a Reflection page). Before
  // the scroll below, which cannot scroll to what a closed fold hides.
  // The location's fragment opens its fold once: a source change re-runs
  // this only until the target exists, so a fold the reader closed again
  // stays closed while the text changes underneath.
  // Keyed by the document too: a link to the same fragment on another
  // document reuses this renderer, and its fold must open. The key is the
  // document drawn (its page address), not the router's path: while the next
  // page loads, the router already names it and this still draws the last
  // one, whose fold the reader may have closed.
  const revealedHash = useRef<string | null>(null);
  const shownPage = anchors?.pageUrl ?? pathname;
  useEffect(() => {
    const at = `${shownPage}${hash}`;
    if (revealedHash.current === at) {
      return;
    }
    if (revealFragment(rootRef.current, hash)) {
      revealedHash.current = at;
    }
  }, [hash, shownPage, source]);
  useEffect(() => {
    const root = rootRef.current;
    const onHash = () => {
      revealFragment(root, window.location.hash);
    };
    const onReveal = (event: Event) => {
      const detail = (event as CustomEvent<RevealDetail | undefined>).detail;
      const target =
        detail?.target ??
        (event.target instanceof Element ? event.target : null);
      revealIn(root, target);
    };
    window.addEventListener("hashchange", onHash);
    root?.addEventListener(REVEAL_EVENT, onReveal);
    return () => {
      window.removeEventListener("hashchange", onHash);
      root?.removeEventListener(REVEAL_EVENT, onReveal);
    };
  }, []);
  // Arriving at a section: scroll it into view and point it out for a moment.
  // It runs on the hash rather than on a navigation, which is what makes a
  // click on a link symbol behave exactly like opening the URL it copied, and
  // on the source too, because a document that just changed underneath a
  // reader has different headings.
  useEffect(() => {
    if (!anchored || hash.length < 2) {
      return;
    }
    let id = hash.slice(1);
    try {
      id = decodeURIComponent(id);
    } catch {
      // A hash that is not encoded stays as it is.
    }
    // `getElementById` rather than a selector: a slug keeps the letters the
    // heading was written with, and a selector would need those escaped
    // through a `CSS.escape` this app cannot assume.
    const target = document.getElementById(id);
    if (!target || !rootRef.current?.contains(target)) {
      // A fragment naming nothing in this document does nothing at all: no
      // scroll, no message, the page opens where it always did.
      return;
    }
    target.scrollIntoView({ block: "start" });
    target.classList.remove(SECTION_FLASH_CLASS);
    // Reading a layout value between the two is what restarts the animation
    // when this effect points the same heading out twice, which happens when
    // the same fragment arrives again from outside. A second click on the
    // same link symbol leaves the hash where it already was, so it copies and
    // announces again without re-flashing what the reader is already at.
    void target.offsetWidth;
    target.classList.add(SECTION_FLASH_CLASS);
    const timer = setTimeout(() => {
      target.classList.remove(SECTION_FLASH_CLASS);
    }, SECTION_FLASH_MS);
    return () => {
      clearTimeout(timer);
      // And take the wash off the heading being left. Under reduced motion the
      // class IS a static background rather than a fade, so a second fragment
      // arriving inside the window would otherwise leave the first heading
      // highlighted for as long as the page stays open.
      target.classList.remove(SECTION_FLASH_CLASS);
    };
  }, [anchored, hash, source]);
  const rendered = (
    <ReactMarkdown
      remarkPlugins={remarkPlugins}
      rehypePlugins={rehypePlugins}
      components={componentMap}
      urlTransform={urlTransform}
      remarkRehypeOptions={{ clobberPrefix: `user-content-${noteScope}-` }}
    >
      {shown}
    </ReactMarkdown>
  );
  return (
    // `measured` here rather than on a page wrapper: the rule is
    // `.measured > :not(.breakout)`, so it has to sit on the one element
    // whose direct children are the document's own blocks. A wrapper around
    // this renderer would see a single child - this div - and cap the tables
    // and diagrams inside it along with the prose. Hardcoded rather than a
    // prop, because every markdown surface is a reading surface.
    <div ref={rootRef} className="measured text-body">
      <TitleNoteContext.Provider value={titleNote}>
        <HeadingAsideContext.Provider value={headingAside}>
          <TaskTickContext.Provider value={taskTick}>
            <NoteLinksContext.Provider value={wikilinks}>
              {anchors ? (
                <SectionAnchorProvider pageUrl={anchors.pageUrl}>
                  {rendered}
                  <SectionLinkStatus />
                </SectionAnchorProvider>
              ) : (
                rendered
              )}
            </NoteLinksContext.Provider>
          </TaskTickContext.Provider>
        </HeadingAsideContext.Provider>
      </TitleNoteContext.Provider>
    </div>
  );
}
