/**
 * Rich text from another app, as clean Markdown in this dialect.
 *
 * Clipboard HTML is untrusted. It is parsed into an inert document
 * (`DOMParser`, which runs no script and loads nothing), active elements and
 * event attributes are removed, and the shapes other apps write are rewritten
 * into plain elements before turndown sees them: a wiki's panels become
 * callouts, its expands folded details, its lozenges status labels; a docs
 * editor's styled spans become emphasis; a word processor's list paragraphs
 * become lists. What is not recognised falls back to its text, never to HTML.
 *
 * Loaded only on the first rich paste (`richPaste.ts`), so turndown never
 * reaches the eager bundle.
 */

import { gfm } from "@truto/turndown-plugin-gfm";
import TurndownService from "turndown";

import type { CalloutKind } from "../markdown/callouts";
import { cleanClipboardHtml, decodeEntities } from "./clipboardHtml";
import type { StatusColour } from "../markdown/directives";
import { isStatusColour } from "../markdown/directives";

/*
 * How a wiki writes its panels, expands and status lozenges, in one table.
 * The server-rendered page names its macros in `data-macro-name`; the newer
 * editor writes `data-panel-type` and `data-node-type`. The class names and
 * the lozenge colour modifiers are modelled on that wiki's clipboard and are
 * provisional until a real capture replaces the fixture (spec O3).
 */
const WIKI = {
  /** `data-macro-name` of a panel macro, and the callout it becomes. */
  calloutMacros: {
    info: "note",
    note: "note",
    tip: "tip",
    warning: "warning",
    panel: "note",
  } as Record<string, CalloutKind>,
  /** `data-panel-type` of the newer editor's panel. */
  panelTypes: {
    info: "note",
    note: "note",
    tip: "tip",
    success: "tip",
    warning: "warning",
    error: "caution",
    custom: "note",
  } as Record<string, CalloutKind>,
  /** A panel's class when no macro name came along. */
  panelClass: /(?:^|\s)(?:panel|information-macro)(?:\s|$)/,
  /** Where a panel keeps its title; removed from the body. */
  panelTitle: ":scope > .panelHeader, :scope > .title",
  expand:
    '[data-macro-name="expand"], .expand-container, [data-node-type="expand"]',
  expandTitle: ".expand-control-text",
  /** The expand's clickable head, removed from the body with its title. */
  expandControl: ":scope > .expand-control",
  status:
    '[data-macro-name="status"], .status-macro, [class*="lozenge"], [data-node-type="status"]',
  /** The colour modifier on a lozenge's class, or the newer `data-color`. */
  statusModifier: /(?:^|[\s-])lozenge-([a-z]+)/g,
  statusColours: {
    gray: "grey",
    neutral: "grey",
    success: "green",
    complete: "blue",
    error: "red",
    current: "yellow",
    moved: "yellow",
    new: "purple",
  } as Record<string, StatusColour>,
  taskList: "ul.inline-task-list > li",
};

/**
 * Whether the plain text is the better paste: a code editor copies its text
 * as `white-space: pre` lines with coloured spans, and turndown would fold
 * the indentation away.
 */
export function prefersPlainText(html: string): boolean {
  const body = cleanClipboardHtml(html).body;
  const blocks = Array.from(body.children).filter((el) => el.tagName !== "BR");
  return (
    blocks.length > 0 &&
    blocks.every(
      (el) =>
        el instanceof HTMLElement &&
        el.tagName === "DIV" &&
        el.style.whiteSpace.trim() === "pre",
    )
  );
}

/** Rich text from the clipboard as Markdown in this dialect. */
export function htmlToMarkdown(html: string): string {
  const doc = cleanClipboardHtml(html);
  rewrite(doc);
  return service().turndown(doc.body);
}

// ---------------------------------------------------------------------------
// Rewriting other apps' shapes into plain elements

/** Marks an element the callout rule turns into `> [!KIND]`. */
const CALLOUT = "data-paste-callout";
const CALLOUT_TITLE = "data-paste-title";
/** Marks an element the status rule turns into `:status[...]{colour}`. */
const STATUS = "data-paste-status";

function rewrite(doc: Document): void {
  // Images first: the links an image becomes are checked with the rest.
  images(doc);
  links(doc);
  callouts(doc);
  statuses(doc);
  styledSpans(doc);
  wordLists(doc);
  taskLists(doc);
  listParagraphs(doc);
  bareCode(doc);
}

/** The schemes a link keeps; anything else keeps only its text. */
const LINK_SCHEMES = ["http", "https", "mailto"];
const IMAGE_SCHEMES = ["http", "https"];

/**
 * Whether `raw` is a relative URL or names one of `schemes`. Read the way a
 * browser and a Markdown renderer read it: character references decoded
 * (however often they were encoded), control characters and whitespace not
 * counted, the scheme matched without regard to case.
 */
function isSafeUrl(raw: string, schemes: string[]): boolean {
  const probe = decodeEntities(raw)
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u0020\u007f-\u009f]/g, "")
    .toLowerCase();
  const scheme = /^([a-z][a-z0-9+.-]*):/.exec(probe)?.[1];
  return scheme === undefined || schemes.includes(scheme);
}

/**
 * A URL as a Markdown link destination that reads back as the same URL: a
 * backslash is escaped, and so is an `&` that would start a character
 * reference (`&#106;`, `&amp;`), which a renderer would decode. A plain `&`
 * between query parameters stays as it is. Turndown escapes the parentheses.
 */
function destination(url: string): string {
  return url
    .trim()
    .replace(/\\/g, "\\\\")
    .replace(/&(?=#?[A-Za-z0-9]+;)/g, "\\&");
}

function unwrap(el: Element): void {
  el.replaceWith(...Array.from(el.childNodes));
}

function links(doc: Document): void {
  for (const a of Array.from(doc.querySelectorAll("a"))) {
    const href = a.getAttribute("href");
    if (href === null || !isSafeUrl(href, LINK_SCHEMES)) {
      unwrap(a);
    } else {
      a.setAttribute("href", destination(href));
    }
  }
}

/** An image becomes a link to it: the person attaches files separately. */
function images(doc: Document): void {
  for (const img of Array.from(doc.querySelectorAll("img"))) {
    const src = img.getAttribute("src")?.trim() ?? "";
    const alt = img.getAttribute("alt")?.trim() ?? "";
    if (
      src !== "" &&
      isSafeUrl(src, IMAGE_SCHEMES) &&
      img.closest("a") === null
    ) {
      const a = doc.createElement("a");
      a.setAttribute("href", src);
      a.textContent = alt === "" ? "image" : alt;
      img.replaceWith(a);
    } else {
      img.replaceWith(doc.createTextNode(alt));
    }
  }
}

function calloutKind(node: Element): CalloutKind | null {
  const macro = node.getAttribute("data-macro-name");
  if (macro !== null) {
    return WIKI.calloutMacros[macro] ?? null;
  }
  const panel = node.getAttribute("data-panel-type");
  if (panel !== null) {
    return WIKI.panelTypes[panel] ?? "note";
  }
  const cls = node.getAttribute("class") ?? "";
  if (!WIKI.panelClass.test(cls)) {
    return null;
  }
  const kind = /information-macro-(information|note|tip|warning)/.exec(
    cls,
  )?.[1];
  return kind === "tip" || kind === "warning" ? kind : "note";
}

function oneLine(text: string | null | undefined): string {
  return (text ?? "").replace(/\s+/g, " ").trim();
}

/**
 * Panels and expands become callouts. Document order visits the outer one
 * first, and one inside a callout stays its plain content: callouts never
 * nest.
 */
function callouts(doc: Document): void {
  const candidates = Array.from(
    doc.querySelectorAll(
      `${WIKI.expand}, [data-macro-name], [data-panel-type], .panel, .information-macro`,
    ),
  );
  for (const el of candidates) {
    if (el.parentElement?.closest(`[${CALLOUT}]`)) {
      continue;
    }
    if (el.matches(WIKI.expand)) {
      const title =
        oneLine(el.querySelector(WIKI.expandTitle)?.textContent) ||
        oneLine(el.getAttribute("data-title"));
      el.querySelector(WIKI.expandControl)?.remove();
      mark(el, "details", title || "Details");
      continue;
    }
    const kind = calloutKind(el);
    if (kind === null) {
      continue;
    }
    const titleEl = el.querySelector(WIKI.panelTitle);
    const title = oneLine(titleEl?.textContent);
    titleEl?.remove();
    mark(el, kind, title);
  }
}

function mark(el: Element, kind: CalloutKind, title: string): void {
  el.setAttribute(CALLOUT, kind);
  el.setAttribute(CALLOUT_TITLE, title);
}

/** The palette colour nearest to what a lozenge says about itself. */
function lozengeColour(el: Element): StatusColour {
  const named = [
    el.getAttribute("data-color") ?? "",
    ...Array.from(
      (el.getAttribute("class") ?? "").matchAll(WIKI.statusModifier),
      (m) => m[1] ?? "",
    ),
  ];
  for (const word of named.map((w) => w.toLowerCase())) {
    if (isStatusColour(word)) {
      return word;
    }
    const mapped = WIKI.statusColours[word];
    if (mapped !== undefined) {
      return mapped;
    }
  }
  return "grey";
}

function statuses(doc: Document): void {
  for (const el of Array.from(doc.querySelectorAll(WIKI.status))) {
    if (el.closest(`[${STATUS}]`)) {
      continue;
    }
    el.setAttribute(STATUS, lozengeColour(el));
  }
}

/**
 * A docs editor writes emphasis as inline styles on spans, and wraps the
 * whole copy in a `<b>` whose style says it is not bold.
 */
function styledSpans(doc: Document): void {
  for (const b of Array.from(doc.querySelectorAll<HTMLElement>("b, strong"))) {
    const weight = b.style.fontWeight;
    if (weight === "normal" || weight === "400") {
      unwrap(b);
    }
  }
  for (const span of Array.from(
    doc.querySelectorAll<HTMLElement>("span[style]"),
  )) {
    const style = span.style;
    const wrappers: string[] = [];
    if (style.fontWeight === "bold" || Number(style.fontWeight) >= 600) {
      wrappers.push("strong");
    }
    if (style.fontStyle === "italic") {
      wrappers.push("em");
    }
    if (/line-through/.test(style.textDecoration)) {
      wrappers.push("del");
    }
    for (const tag of wrappers) {
      const wrapper = doc.createElement(tag);
      wrapper.append(...Array.from(span.childNodes));
      span.append(wrapper);
    }
  }
}

/** A word processor's list paragraph: its list, its level and its marker. */
interface ListParagraph {
  p: HTMLElement;
  /** Which list it belongs to (`l0`, `l1`): the next list starts a new run. */
  list: string;
  level: number;
  ordered: boolean;
  /** The span holding the bullet or number, which the list replaces. */
  marker: Element | null;
}

function listParagraph(node: Node | null): ListParagraph | null {
  if (!(node instanceof HTMLElement) || node.tagName !== "P") {
    return null;
  }
  const style = node.getAttribute("style") ?? "";
  const list = /mso-list:\s*(l\d+)/i.exec(style)?.[1];
  if (list === undefined) {
    return null;
  }
  const level = Number(/level(\d+)/i.exec(style)?.[1] ?? "1");
  const marker = node.querySelector('[style*="mso-list:Ignore" i]');
  const ordered = /^[0-9a-z]{1,4}[.)]$/i.test(oneLine(marker?.textContent));
  return { p: node, list, level: Math.max(1, level), ordered, marker };
}

/** The sibling after `node`, past whitespace and comments. */
function nextElementLike(node: Node): Node | null {
  let next = node.nextSibling;
  while (
    next !== null &&
    (next.nodeType === Node.COMMENT_NODE ||
      (next.nodeType === Node.TEXT_NODE && oneLine(next.textContent) === ""))
  ) {
    next = next.nextSibling;
  }
  return next;
}

/**
 * A word processor writes a list as paragraphs with an `mso-list` style and
 * the bullet as text inside a span it marks `mso-list:Ignore`. A run of them
 * becomes a real list, nested by level.
 */
function wordLists(doc: Document): void {
  const seen = new Set<Element>();
  for (const p of Array.from(doc.querySelectorAll("p[style]"))) {
    if (seen.has(p)) {
      continue;
    }
    const first = listParagraph(p);
    if (first === null) {
      continue;
    }
    const run = [first];
    let next = nextElementLike(p);
    for (
      let item = listParagraph(next);
      item !== null && item.list === first.list;
      item = listParagraph(next)
    ) {
      run.push(item);
      next = nextElementLike(item.p);
    }
    const root = doc.createElement(first.ordered ? "ol" : "ul");
    const stack: HTMLElement[] = [root];
    for (const item of run) {
      seen.add(item.p);
      while (stack.length > item.level) {
        stack.pop();
      }
      while (stack.length < item.level) {
        const list = doc.createElement(item.ordered ? "ol" : "ul");
        const parent = stack.at(-1) as HTMLElement;
        (parent.lastElementChild ?? parent).append(list);
        stack.push(list);
      }
      item.marker?.remove();
      const li = doc.createElement("li");
      li.append(...Array.from(item.p.childNodes));
      (stack.at(-1) as HTMLElement).append(li);
    }
    p.before(root);
    for (const item of run) {
      item.p.remove();
    }
  }
}

/** Puts a checkbox in front of an item, which the task list rule reads. */
function checkbox(doc: Document, li: Element, checked: boolean): void {
  const input = doc.createElement("input");
  input.type = "checkbox";
  input.checked = checked;
  li.prepend(input);
}

/** A wiki's task list and a docs editor's checklist become task items. */
function taskLists(doc: Document): void {
  for (const li of Array.from(doc.querySelectorAll(WIKI.taskList))) {
    checkbox(doc, li, li.classList.contains("checked"));
  }
  for (const li of Array.from(doc.querySelectorAll('li[role="checkbox"]'))) {
    checkbox(doc, li, li.getAttribute("aria-checked") === "true");
  }
}

/**
 * An item that is one paragraph is the item's text: a docs editor wraps every
 * item in a `<p>`, which would part a checkbox from its words. The space
 * after a checkbox goes, so the item reads `- [x] done`.
 */
function listParagraphs(doc: Document): void {
  for (const li of Array.from(doc.querySelectorAll("li"))) {
    const elements = Array.from(li.children).filter(
      (el) => el.tagName !== "INPUT",
    );
    if (elements.length === 1 && elements[0]?.tagName === "P") {
      unwrap(elements[0]);
    }
    const box = li.firstElementChild;
    if (
      box?.tagName === "INPUT" &&
      box.nextSibling?.nodeType === Node.TEXT_NODE
    ) {
      box.nextSibling.textContent = (box.nextSibling.textContent ?? "").replace(
        /^\s+/,
        "",
      );
    }
  }
}

/** A `<pre>` with no `<code>` inside is still a code block. */
function bareCode(doc: Document): void {
  for (const pre of Array.from(doc.querySelectorAll("pre"))) {
    if (
      pre.firstElementChild?.tagName === "CODE" &&
      pre.childNodes.length === 1
    ) {
      continue;
    }
    const code = doc.createElement("code");
    code.textContent = pre.textContent;
    pre.replaceChildren(code);
  }
}

// ---------------------------------------------------------------------------
// Turndown and its rules

let instance: TurndownService | null = null;

function service(): TurndownService {
  if (instance !== null) {
    return instance;
  }
  const td = new TurndownService({
    headingStyle: "atx",
    hr: "---",
    bulletListMarker: "-",
    codeBlockStyle: "fenced",
    emDelimiter: "*",
    strongDelimiter: "**",
  });
  td.escape = escapeText;
  td.use(gfm);
  // After the plugin, so these replace its cell rule.
  td.addRule("tableCell", { filter: ["th", "td"], replacement: tableCell });
  td.addRule("listItem", { filter: "li", replacement: listItem });
  td.addRule("callout", {
    filter: (node) => node.hasAttribute(CALLOUT),
    replacement: callout,
  });
  td.addRule("status", {
    filter: (node) => node.hasAttribute(STATUS),
    replacement: (_content, node) => {
      // The brackets go: a label ends at its first `]`, for the core's lint
      // as much as for the renderer. The rest is escaped like any text.
      const text = escapeText(oneLine(node.textContent).replace(/[[\]]/g, ""));
      return text === ""
        ? ""
        : `:status[${text}]{${node.getAttribute(STATUS)}}`;
    },
  });
  instance = td;
  return td;
}

/**
 * One table cell. The plugin's own rule escapes a pipe and then doubles every
 * backslash, which breaks the row at the pipe and doubles turndown's own
 * escapes; this escapes each pipe once, by what comes before it.
 */
function tableCell(content: string, node: TurndownService.Node): string {
  const el = node as HTMLElement;
  const index = el.parentElement
    ? Array.prototype.indexOf.call(el.parentElement.children, el)
    : 0;
  let text = escapeCellPipes(content.replace(/\s+/g, " ").trim());
  text = text.padEnd(3, " ");
  const span = Math.max(
    1,
    Number.parseInt(el.getAttribute("colspan") ?? "1", 10) || 1,
  );
  return `${index === 0 ? "| " : " "}${text} |${"   |".repeat(span - 1)}`;
}

/**
 * The pipes in a cell, escaped so that none of them ends the cell.
 *
 * A table row is split before any inline parsing, and the split reads a
 * backslash as an escape of the next backslash or pipe. So a pipe ends the
 * cell when an even run of backslashes (or none) comes before it, and
 * escaping it means one more backslash. Escaping every backslash first, as
 * for plain text, would double turndown's own escapes: the text in `content`
 * is escaped already (`escapeText` doubles every backslash in it), so its
 * runs are always even. An odd run comes only from text turndown does not
 * escape, a code span: there `\|` already keeps the cell whole, but the
 * parser then drops that backslash from the code, so the run gets two more
 * and the code reads back with its backslash (doubled; a code span in a table
 * cannot hold a backslash right before a pipe, see mdast-util-gfm-table).
 */
function escapeCellPipes(text: string): string {
  return text.replace(/(\\*)\|/g, (_pipe: string, run: string) =>
    run.length % 2 === 0 ? `${run}\\|` : `${run}\\\\|`,
  );
}

/**
 * One list item, with one space after the marker (turndown writes three) and
 * continuation lines indented to the item's text.
 */
function listItem(
  content: string,
  node: TurndownService.Node,
  options: TurndownService.Options,
): string {
  const el = node as HTMLElement;
  const parent = el.parentElement;
  let prefix = `${options.bulletListMarker ?? "-"} `;
  if (parent?.tagName === "OL") {
    const start = Number(parent.getAttribute("start") ?? "1") || 1;
    prefix = `${start + Array.prototype.indexOf.call(parent.children, el)}. `;
  }
  const loose = /\n$/.test(content);
  const body =
    content.replace(/^\n+/, "").replace(/\n+$/, "") + (loose ? "\n" : "");
  const indented = body.replace(
    /\n(?=[^\n])/g,
    `\n${" ".repeat(prefix.length)}`,
  );
  return prefix + indented + (el.nextSibling ? "\n" : "");
}

/** A callout: the head line, then every body line quoted. */
function callout(content: string, node: TurndownService.Node): string {
  const el = node as HTMLElement;
  const kind = el.getAttribute(CALLOUT) ?? "note";
  const title = el.getAttribute(CALLOUT_TITLE) ?? "";
  const fold = kind === "details" ? "-" : "";
  const head = `[!${kind.toUpperCase()}]${fold}${title === "" ? "" : ` ${escapeText(title)}`}`;
  const body = content.replace(/^\n+/, "").replace(/\n+$/, "");
  const lines = [head, ...(body === "" ? [] : body.split("\n"))];
  return `\n\n${lines.map((line) => (line.trim() === "" ? ">" : `> ${line}`)).join("\n")}\n\n`;
}

/** A letter or a digit in any script: what an intraword `_` sits between. */
const WORD_CHAR = /[\p{L}\p{N}]/u;

/**
 * Pasted text as Markdown text that reads back as the same text.
 *
 * Replaces turndown's own escape for two reasons. It never escaped `<`, so
 * visible text such as `<script>` or `<!-- r:... -->` became raw HTML (or a
 * hidden comment, or a forged section id) in the source. And it escaped
 * every `_`, so `snake_case` read `snake\_case`. Only what changes meaning is
 * escaped here: a `<` that would open a tag or a comment, an `&` that would
 * start a character reference, a `_` at the edge of a word, and what starts
 * a block at the start of the text.
 */
export function escapeText(text: string): string {
  return text
    .replace(/[\\*`[\]]/g, "\\$&")
    .replace(/<(?=[A-Za-z!/?])/g, "\\<")
    .replace(/&(?=#?[A-Za-z0-9]+;)/g, "\\&")
    .replace(/~~/g, "\\~\\~")
    .replace(/\$\$/g, () => "\\$\\$")
    .replace(/_/g, (underscore: string, at: number, all: string) =>
      WORD_CHAR.test(all.charAt(at - 1)) && WORD_CHAR.test(all.charAt(at + 1))
        ? underscore
        : "\\_",
    )
    .replace(/^-/, "\\-")
    .replace(/^\+ /, "\\+ ")
    .replace(/^(=+)/, "\\$1")
    .replace(/^(#{1,6}) /, "\\$1 ")
    .replace(/^>/, "\\>")
    .replace(/^(\d+)\. /, "$1\\. ");
}
