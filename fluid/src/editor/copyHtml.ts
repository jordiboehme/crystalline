/**
 * Copy and cut put HTML on the clipboard beside the Markdown, so a paste into
 * a mail or a document keeps the formatting.
 *
 * `copyHtmlOf` reads the document's own syntax tree over the selected ranges,
 * so every selected character keeps the formatting it has in the document,
 * even when the selection holds only one mark of a pair. Its output is built
 * from the person's text and is untrusted: every text node and attribute
 * value is escaped, the only attributes ever written are `href` (http, https
 * and mailto only) and `start` on `ol`, and raw HTML in the source is written
 * as text. HTML comments and Reflection id lines are dropped.
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, SelectionRange } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";
import { decodeNamedCharacterReference } from "decode-named-character-reference";

import { parseCalloutHead } from "../markdown/callouts";
import { statusLabelWords } from "../markdown/statusText";
import { NOT_APPLICABLE } from "../markdown/taskState";
import { isIdLine } from "../reflectionText";
import { WIKILINK, wikilinkWords } from "../wikilinks";
import { footnoteNumberAt } from "./footnotes";
import { frontmatterRegion } from "./frontmatterRegion";
import { mathBlocks } from "./mathBlocks";

/** The attribute on the root of the HTML a copy writes (see `isOwnCopy`). */
const COPY_MARKER = 'data-crystalline-copy="1"';

const ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

const esc = (text: string): string =>
  text.replace(/[&<>"']/g, (c) => ESCAPES[c] ?? c);

/**
 * The backslash escapes and character references the parser reads in an
 * address or a text, with an unknown or out-of-range one left as it is.
 */
function decode(text: string): string {
  return text.replace(
    /\\([!-/:-@[-`{-~])|&(?:#(\d+)|#[xX]([0-9a-fA-F]+)|(\w+));/g,
    (whole, punct?: string, dec?: string, hex?: string, name?: string) => {
      if (punct !== undefined) {
        return punct;
      }
      if (name !== undefined) {
        return decodeNamedCharacterReference(name) || whole;
      }
      const code = dec !== undefined ? Number(dec) : parseInt(hex ?? "", 16);
      return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : whole;
    },
  );
}

/** An address a link may carry: http, https or mailto, else null. */
function safeHref(raw: string): string | null {
  // A browser drops tabs and line breaks inside an address before it reads
  // the scheme, so they are dropped here before the scheme is checked.
  const value = decode(raw)
    .replace(/^\s*<|>\s*$/g, "")
    .trim()
    .replace(/[\t\r\n]/g, "");
  return /^(?:https?|mailto):/i.test(value) ? value : null;
}

const wrap = (tag: string, inner: string, attrs = ""): string =>
  inner === "" ? "" : `<${tag}${attrs}>${inner}</${tag}>`;

const tidy = (html: string): string => html.trim();

/** One piece of an inline run: a syntax node, or a wikilink the text holds. */
interface Item {
  name: string;
  from: number;
  to: number;
  node: SyntaxNode | null;
  inner: string;
}

const SKIPPED = /Mark|Delim|Title|Label|Comment|Instr|Reference/;
const CODEISH = /^(?:InlineCode|InlineMath|HTMLTag|URL)$/;

const kids = (node: SyntaxNode): SyntaxNode[] => {
  const out: SyntaxNode[] = [];
  for (let c = node.firstChild; c; c = c.nextSibling) {
    out.push(c);
  }
  return out;
};

/** The HTML for the selected `ranges` of `state`. */
export function copyHtmlOf(
  state: EditorState,
  ranges: readonly SelectionRange[],
): string {
  const end = ranges.reduce((to, r) => Math.max(to, r.to), 0);
  void end;
  const tree =
    ensureSyntaxTree(state, state.doc.length, 200) ?? syntaxTree(state);
  const maths = mathBlocks(state).filter((m) => m.form === "dollars");
  const front = frontmatterRegion(state.doc);

  const txt = (from: number, to: number): string =>
    from < to ? esc(state.sliceDoc(from, to)) : "";
  const whole = (c: { from: number; to: number }, lo: number, hi: number) =>
    c.from >= lo && c.to <= hi;

  const norm = (key: string) => key.trim().replace(/\s+/g, " ").toLowerCase();
  let defs: Map<string, string> | undefined;
  /** The address a reference label stands for, as the document defines it. */
  function defined(key: string): string | undefined {
    if (!defs) {
      const found = new Map<string, string>();
      tree.iterate({
        enter(n) {
          if (n.name !== "LinkReference") {
            return;
          }
          const label = n.node.getChild("LinkLabel");
          const url = n.node.getChild("URL");
          const name = label
            ? norm(state.sliceDoc(label.from + 1, label.to - 1))
            : null;
          if (name !== null && url && !found.has(name)) {
            found.set(name, state.sliceDoc(url.from, url.to));
          }
          return false;
        },
      });
      defs = found;
    }
    return defs.get(norm(key));
  }

  function items(n: SyntaxNode): Item[] {
    const real = kids(n).map((c): Item => ({
      name: c.name,
      from: c.from,
      to: c.to,
      node: c,
      inner: "",
    }));
    const text = state.sliceDoc(n.from, n.to);
    if (!text.includes("[[")) {
      return real;
    }
    let out = real;
    for (const m of text.matchAll(WIKILINK)) {
      const from = n.from + m.index;
      const to = from + m[0].length;
      const hit = (c: Item) => c.from < to && c.to > from;
      if (out.some((c) => hit(c) && CODEISH.test(c.name))) {
        continue;
      }
      out = out.filter((c) => !hit(c));
      out.push({ name: "Wikilink", from, to, node: null, inner: m[1] ?? "" });
    }
    return out.sort((a, b) => a.from - b.from);
  }

  /** The content of `n` inside [lo, hi). */
  function inl(n: SyntaxNode, lo: number, hi: number): string {
    const from = Math.max(lo, n.from);
    const to = Math.min(hi, n.to);
    let out = "";
    let pos = from;
    for (const c of items(n)) {
      if (c.to <= from) {
        continue;
      }
      if (c.from >= to) {
        break;
      }
      out += txt(pos, Math.min(c.from, to)) + one(c, from, to);
      pos = Math.max(pos, c.to);
    }
    return out + txt(pos, to);
  }

  const link = (href: string | null, words: string): string =>
    href === null ? words : wrap("a", words, ` href="${esc(href)}"`);

  function one(item: Item, lo: number, hi: number): string {
    const raw = () => txt(Math.max(item.from, lo), Math.min(item.to, hi));
    if (item.node === null) {
      return whole(item, lo, hi)
        ? esc(wikilinkWords(item.inner) ?? state.sliceDoc(item.from, item.to))
        : raw();
    }
    const c = item.node;
    const marks = kids(c).filter((k) => k.name === "LinkMark");
    const url = kids(c).find((k) => k.name === "URL");
    switch (c.name) {
      case "Escape":
        return txt(Math.max(c.from + 1, lo), Math.min(c.to, hi));
      case "Entity":
        return whole(c, lo, hi)
          ? esc(decode(state.sliceDoc(c.from, c.to)))
          : raw();
      case "HardBreak":
        return "<br>";
      case "StrongEmphasis":
        return wrap("strong", inl(c, lo, hi));
      case "Emphasis":
        return wrap("em", inl(c, lo, hi));
      case "Strikethrough":
        return wrap("del", inl(c, lo, hi));
      case "InlineCode":
      case "InlineMath": {
        const m = kids(c).filter((k) => /Mark/.test(k.name));
        const from = m.length > 1 ? (m[0]?.to ?? c.from) : c.from;
        const to = m.length > 1 ? (m[m.length - 1]?.from ?? c.to) : c.to;
        return wrap("code", txt(Math.max(from, lo), Math.min(to, hi)));
      }
      case "StatusLabel":
        return whole(c, lo, hi)
          ? esc(statusLabelWords(state.sliceDoc(c.from, c.to)))
          : raw();
      case "Image": {
        const [open, close] = marks;
        return open
          ? txt(Math.max(open.to, lo), Math.min(close?.from ?? c.to, hi))
          : "";
      }
      case "Autolink":
      case "URL": {
        const address = c.name === "URL" ? c : url;
        const words = address
          ? txt(Math.max(address.from, lo), Math.min(address.to, hi))
          : raw();
        return link(
          address ? safeHref(state.sliceDoc(address.from, address.to)) : null,
          words,
        );
      }
      case "Link": {
        if (state.sliceDoc(c.from, c.from + 2) === "[^") {
          const number = footnoteNumberAt(state, c.from, c.to);
          return number !== null && whole(c, lo, hi)
            ? `<sup>${number}</sup>`
            : raw();
        }
        const [open, close] = marks;
        if (!open || !close) {
          return raw();
        }
        const label = kids(c).find((k) => k.name === "LinkLabel");
        let key = label ? state.sliceDoc(label.from + 1, label.to - 1) : "";
        if (key.trim() === "") {
          key = state.sliceDoc(open.to, close.from);
        }
        const address = url ? state.sliceDoc(url.from, url.to) : defined(key);
        const inner = inl(c, Math.max(lo, open.to), Math.min(hi, close.from));
        if (address === undefined) {
          // No address anywhere: the reader shows the brackets, so they stay.
          return (
            txt(Math.max(open.from, lo), Math.min(open.to, hi)) +
            inner +
            txt(Math.max(close.from, lo), Math.min(c.to, hi))
          );
        }
        return link(safeHref(address), inner);
      }
      case "Task":
        return box(c, lo, hi) + inl(c, lo, hi);
      default:
        return SKIPPED.test(c.name) ? "" : raw();
    }
  }

  function box(task: SyntaxNode, lo: number, hi: number): string {
    const marker = kids(task).find((k) => k.name === "TaskMarker");
    if (!marker || marker.to <= lo || marker.from >= hi) {
      return "";
    }
    return /\[[xX]\]/.test(state.sliceDoc(marker.from, marker.to)) ? "☑" : "☐";
  }

  /** A paragraph's words, with the `[~]` box of a list item's first paragraph. */
  function words(p: SyntaxNode, lo: number, hi: number, first: boolean) {
    const na = first
      ? NOT_APPLICABLE.exec(state.sliceDoc(p.from, p.from + 12))
      : null;
    if (na) {
      const shown = p.from >= lo && p.from < hi;
      return (
        (shown ? "☒ " : "") + inl(p, Math.max(lo, p.from + na[0].length), hi)
      );
    }
    return inl(p, lo, hi);
  }

  function callout(p: SyntaxNode, mark: SyntaxNode, lo: number, hi: number) {
    const head = parseCalloutHead(
      state.sliceDoc(mark.from, state.doc.lineAt(mark.from).to),
    );
    if (head === null) {
      return wrap("p", tidy(inl(p, lo, hi)));
    }
    const rest = inl(p, Math.max(lo, mark.to + (head.fold ? 1 : 0)), hi);
    const cut = rest.indexOf("\n");
    const title = tidy(cut === -1 ? rest : rest.slice(0, cut));
    const body = cut === -1 ? "" : tidy(rest.slice(cut + 1));
    const kind = head.kind.charAt(0).toUpperCase() + head.kind.slice(1);
    return (
      (mark.to > lo ? wrap("p", wrap("strong", kind)) : "") +
      wrap("p", title) +
      wrap("p", body)
    );
  }

  function row(r: SyntaxNode, tag: string, lo: number, hi: number): string {
    const cells = kids(r).filter(
      (c) => c.name === "TableCell" && c.to > lo && c.from < hi,
    );
    return wrap(
      "tr",
      cells.map((c) => `<${tag}>${tidy(inl(c, lo, hi))}</${tag}>`).join(""),
    );
  }

  function table(t: SyntaxNode, lo: number, hi: number): string {
    const inWindow = kids(t).filter((c) => c.to > lo && c.from < hi);
    const head = inWindow
      .filter((c) => c.name === "TableHeader")
      .map((c) => row(c, "th", lo, hi))
      .join("");
    const body = inWindow
      .filter((c) => c.name === "TableRow")
      .map((c) => row(c, "td", lo, hi))
      .join("");
    return wrap("table", wrap("thead", head) + wrap("tbody", body));
  }

  function item(li: SyntaxNode, lo: number, hi: number): string {
    let out = "";
    let first = true;
    for (const c of kids(li)) {
      if (c.to <= lo || c.from >= hi || c.name === "ListMark") {
        continue;
      }
      if (c.name === "Task" || c.name === "Paragraph") {
        const text = tidy(
          c.name === "Task"
            ? box(c, lo, hi) + inl(c, lo, hi)
            : words(c, lo, hi, first),
        );
        out += first ? text : wrap("p", text);
        first = false;
      } else {
        out += block(c, lo, hi);
      }
    }
    return out;
  }

  function list(l: SyntaxNode, lo: number, hi: number): string {
    const items = kids(l).filter(
      (c) => c.name === "ListItem" && c.to > lo && c.from < hi,
    );
    const body = items.map((li) => `<li>${item(li, lo, hi)}</li>`).join("");
    if (l.name === "BulletList") {
      return wrap("ul", body);
    }
    const mark = items[0] && kids(items[0]).find((k) => k.name === "ListMark");
    const start = mark ? parseInt(state.sliceDoc(mark.from, mark.to), 10) : 1;
    return wrap("ol", body, start > 1 ? ` start="${start}"` : "");
  }

  function code(c: SyntaxNode, lo: number, hi: number): string {
    const text = kids(c)
      .filter((k) => k.name === "CodeText" && k.to > lo && k.from < hi)
      .map((k) => state.sliceDoc(Math.max(k.from, lo), Math.min(k.to, hi)))
      .join("")
      .replace(/\n$/, "");
    return wrap("pre", wrap("code", esc(text)));
  }

  function block(c: SyntaxNode, lo: number, hi: number): string {
    const heading = /^(?:ATX|Setext)Heading(\d)$/.exec(c.name);
    if (heading) {
      return wrap(`h${heading[1]}`, tidy(inl(c, lo, hi)));
    }
    switch (c.name) {
      case "Paragraph": {
        const tex = maths.find((m) => m.from <= c.from && c.to <= m.to);
        if (tex) {
          const part = state
            .sliceDoc(Math.max(lo, c.from), Math.min(hi, c.to))
            .split("\n")
            .map((l) => l.replace(/^[\s>]*/, ""))
            .filter((l) => l.trim() !== "$$")
            .join("\n");
          return wrap(
            "pre",
            wrap("code", esc(lo <= c.from && hi >= c.to ? tex.tex : part)),
          );
        }
        const mark = kids(c).find((k) => k.name === "CalloutMark");
        return mark
          ? callout(c, mark, lo, hi)
          : wrap("p", tidy(words(c, lo, hi, false)));
      }
      case "Blockquote":
        return wrap("blockquote", blocks(c, lo, hi));
      case "BulletList":
      case "OrderedList":
        return list(c, lo, hi);
      case "FencedCode":
      case "CodeBlock":
        return code(c, lo, hi);
      case "HorizontalRule":
        return "<hr>";
      case "Table":
        return table(c, lo, hi);
      default: {
        const text = state.sliceDoc(Math.max(c.from, lo), Math.min(c.to, hi));
        return isIdLine(text) ? "" : wrap("p", tidy(esc(text)));
      }
    }
  }

  function blocks(n: SyntaxNode, lo: number, hi: number): string {
    let out = "";
    for (const c of kids(n)) {
      if (c.to > lo && c.from < hi && !SKIPPED.test(c.name)) {
        out += block(c, lo, hi);
      }
    }
    return out;
  }

  return ranges
    .map((r) => {
      const lo = front ? Math.max(r.from, front.to) : r.from;
      return lo < r.to ? blocks(tree.topNode, lo, r.to) : "";
    })
    .join("");
}

/** Whether the page's selection sits in the view, as CodeMirror asks first. */
function selectionIn(view: EditorView): boolean {
  const sel = view.dom.ownerDocument.defaultView?.getSelection() ?? null;
  const inside = (node: Node | null) =>
    node !== null && view.contentDOM.contains(node);
  return sel !== null && inside(sel.anchorNode) && inside(sel.focusNode);
}

/**
 * Copy and cut for the main view. Where CodeMirror's own copy can run, it
 * does, so its line-wise copy bookkeeping, its selection guard and its cut
 * (`delete.cut`, one undo step, through the same guards as any deletion) stay
 * its own: this handler only slips the HTML beside the text it writes. The
 * HTML root carries `COPY_MARKER`, so a paste in Fluid takes the Markdown.
 * Where it cannot (a view that is not editable must copy and never delete,
 * or the clipboard object is not replaceable), it writes both flavours
 * itself, with the text exactly as CodeMirror would put it: the selected
 * ranges joined with the line break, then the output filters. An empty
 * selection is left to CodeMirror's line-wise copy.
 */
export function copyAsHtml(): Extension {
  const handle =
    (cut: boolean) => (event: ClipboardEvent, view: EditorView) => {
      const { state } = view;
      const ranges = state.selection.ranges.filter((r) => !r.empty);
      const data = event.clipboardData;
      if (ranges.length === 0 || data === null) {
        return false;
      }
      const html = `<div ${COPY_MARKER}>${copyHtmlOf(state, ranges)}</div>`;
      const deletes = cut && !state.readOnly;
      if (!deletes || state.facet(EditorView.editable)) {
        try {
          Object.defineProperty(event, "clipboardData", {
            configurable: true,
            value: {
              clearData: () => data.clearData(),
              setData: (type: string, value: string) => {
                data.setData(type, value);
                if (type === "text/plain") {
                  data.setData("text/html", html);
                }
              },
            },
          });
          return false;
        } catch {
          // Not replaceable: written here.
        }
      }
      if (!selectionIn(view)) {
        return false;
      }
      let text = ranges
        .map((r) => state.sliceDoc(r.from, r.to))
        .join(state.lineBreak);
      for (const filter of state.facet(EditorView.clipboardOutputFilter)) {
        text = filter(text, state);
      }
      event.preventDefault();
      data.clearData();
      data.setData("text/plain", text);
      data.setData("text/html", html);
      if (deletes && state.facet(EditorView.editable)) {
        view.dispatch({
          changes: ranges.map((r) => ({ from: r.from, to: r.to })),
          scrollIntoView: true,
          userEvent: "delete.cut",
        });
      }
      return true;
    };
  return EditorView.domEventHandlers({
    copy: handle(false),
    cut: handle(true),
  });
}
