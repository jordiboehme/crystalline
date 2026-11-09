/**
 * What mermaid's SVG labels leave undone.
 *
 * The app draws SVG text (`htmlLabels: false`, see `mermaid.ts`), and on that
 * path mermaid hands an entity form like `#quot;` to the label as the literal
 * text `&quot;`. Only its HTML labels would have the browser decode it, so a
 * label says `say &quot;hi&quot;` where the author wrote `say #quot;hi#quot;`.
 * `drawnLabels` is the step every surface runs on what `mermaid.render` gives
 * back: it decodes those forms in the drawn text.
 *
 * This runs after mermaid's sanitizer and nothing sanitizes its result, so its
 * safety rests on what it touches and not on what mermaid lets through. It
 * decodes only text whose parent is an SVG-namespace text element (`text`,
 * `tspan`, ...). The HTML serializer always escapes such text, so a label reading
 * `<script>` stays the text `<script>`. The serializer does not escape the text of
 * an HTML `style`, `xmp` and the like, which is why nothing under a
 * `foreignObject` is touched, and an SVG `style` or `script` is left alone too.
 *
 * Mermaid also wraps a long word with no space by its measured width, and it
 * measures the form, so a row of an arrow's label can end inside one: `a&` and
 * `quot;a`. `rejoinSplitForms` puts such a form back on one row before the decode
 * (R59). The drawing cannot tell this from a person's own line break between `a&`
 * and `quot;` on rows as long as a wrapped one, or from a wrap at a space there. It
 * reads those as one form too, which needs a `&` and a whole known name exactly at
 * a row's end, so it is accepted. A short row is never read so.
 *
 * A cost, accepted (R45): by this point a double-escaped form is the same text
 * as a plain one. `&amp;quot;` and `#quot;` both arrive as `&quot;`, so both draw
 * `"`, and a label written the HTML way to show the text `&quot;` shows the
 * character instead. `#38;quot;` or `#35;quot;` show the text.
 *
 * The module is small and only ever loads beside mermaid.
 */

/** The names mermaid's docs and everyday labels use; any other name stays as written. */
const NAMED: Readonly<Record<string, string>> = {
  quot: '"',
  amp: "&",
  lt: "<",
  gt: ">",
  apos: "'",
  nbsp: "\u00A0",
  copy: "\u00A9",
  reg: "\u00AE",
  trade: "\u2122",
  deg: "\u00B0",
  times: "\u00D7",
  divide: "\u00F7",
  plusmn: "\u00B1",
  middot: "\u00B7",
  bull: "\u2022",
  hellip: "\u2026",
  ndash: "\u2013",
  mdash: "\u2014",
  larr: "\u2190",
  rarr: "\u2192",
  uarr: "\u2191",
  darr: "\u2193",
  harr: "\u2194",
  infin: "\u221E",
  ne: "\u2260",
  le: "\u2264",
  ge: "\u2265",
};

/**
 * The character an entity stands for: `name` is `#9829` or `quot`. Null when it
 * stands for none, an unknown name or a number that is no character.
 */
export function entityText(name: string): string | null {
  if (!name.startsWith("#")) {
    return Object.hasOwn(NAMED, name) ? (NAMED[name] ?? null) : null;
  }
  const code = Number(name.slice(1));
  const character =
    Number.isInteger(code) &&
    code > 0 &&
    code <= 0x10ffff &&
    !(code >= 0xd800 && code <= 0xdfff);
  return character ? String.fromCodePoint(code) : null;
}

/** `text` with its entity forms decoded, one pass, as a drawn label's text is. */
export function decodeDrawn(text: string): string {
  return text.replace(
    /&(#\d+|\w+);/g,
    (form, name: string) => entityText(name) ?? form,
  );
}

const SVG_NAMESPACE = "http://www.w3.org/2000/svg";

/** Whether a text node's parent is an element whose text is drawn and always escaped. */
function isDrawnText(parent: Element | null): boolean {
  return (
    parent?.namespaceURI === SVG_NAMESPACE &&
    parent.localName !== "style" &&
    parent.localName !== "script"
  );
}

/** The start of a form at a row's end: `&`, `&qu`, `&#`, `&#12`. */
const FORM_START = /&(#?\w*)$/;
/** The rest of a form at a row's start: `quot;`, `ot;`, `#58;`, `26;`. */
const FORM_REST = /^(#?\w*);/;
/**
 * A row shorter than this was not wrapped, so its end is a person's own line break.
 * Mermaid wraps at 200 px: about 22 characters here, and no glyph of the label font
 * is wide enough to reach that in 10.
 */
const SHORTEST_WRAPPED_ROW = 10;

/** The text nodes of `row` that are drawn text, in order. */
function drawnTextNodes(doc: Document, row: Element): Node[] {
  const nodes: Node[] = [];
  const walker = doc.createTreeWalker(row, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    nodes.push(node);
  }
  return nodes;
}

/** Takes an emptied node's word out, and the row with it if nothing is left. */
function removeEmptied(node: Node, row: Element): void {
  let element: Element | null = node.parentElement;
  node.parentNode?.removeChild(node);
  while (element && element !== row && element.childNodes.length === 0) {
    const parent = element.parentElement;
    element.remove();
    element = parent;
  }
  if (row.textContent === "") {
    row.remove();
  }
}

/**
 * Puts each entity form mermaid's wrapping split across two rows of the label
 * `text` back on the first row, where the decode finds it. Only when the two pieces
 * make a form `entityText` knows, and only in drawn text. Whether anything changed.
 */
function rejoinSplitForms(doc: Document, text: Element): boolean {
  let changed = false;
  const rows = Array.from(text.children).filter(
    (row) => row.namespaceURI === SVG_NAMESPACE,
  );
  for (let i = 0; i + 1 < rows.length; i++) {
    const row = rows[i];
    const next = rows[i + 1];
    if (row === undefined || next === undefined) {
      continue;
    }
    const tail = drawnTextNodes(doc, row).at(-1);
    const head = drawnTextNodes(doc, next)[0];
    if (
      tail === undefined ||
      head === undefined ||
      !isDrawnText(tail.parentElement) ||
      !isDrawnText(head.parentElement) ||
      (row.textContent ?? "").length < SHORTEST_WRAPPED_ROW
    ) {
      continue;
    }
    const start = FORM_START.exec(tail.textContent ?? "");
    const rest = FORM_REST.exec(head.textContent ?? "");
    if (start === null || rest === null) {
      continue;
    }
    const name = `${start[1] ?? ""}${rest[1] ?? ""}`;
    if (!/^(?:#\d+|\w+)$/.test(name) || entityText(name) === null) {
      continue;
    }
    tail.textContent = `${tail.textContent ?? ""}${rest[0]}`;
    const left = (head.textContent ?? "").slice(rest[0].length);
    changed = true;
    if (left !== "") {
      head.textContent = left;
      continue;
    }
    removeEmptied(head, next);
    // The word that now starts the row brought the space that stood before it.
    const first = drawnTextNodes(doc, next)[0];
    if (first !== undefined && isDrawnText(first.parentElement)) {
      first.textContent = (first.textContent ?? "").trimStart();
    }
  }
  return changed;
}

/**
 * `svg` with the forms a wrap split put back together and the entity forms in its
 * drawn text decoded, one pass per text node (see the file header for what counts as drawn text). Styles, attributes,
 * titles and foreign objects stay as they are. An svg with nothing to decode
 * comes back as the same string.
 */
export function drawnLabels(svg: string): string {
  // The text `&quot;` serializes as `&amp;quot;`, so without this there is no form.
  if (!svg.includes("&amp;")) {
    return svg;
  }
  const doc = new DOMParser().parseFromString(svg, "text/html");
  const root = doc.body.firstElementChild;
  if (root?.localName !== "svg" || doc.body.children.length !== 1) {
    return svg;
  }
  let changed = false;
  for (const text of root.querySelectorAll("text")) {
    if (text.namespaceURI !== SVG_NAMESPACE) {
      continue;
    }
    changed = rejoinSplitForms(doc, text) || changed;
    const walker = doc.createTreeWalker(text, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (!isDrawnText(node.parentElement)) {
        continue;
      }
      const before = node.textContent ?? "";
      const after = decodeDrawn(before);
      if (after !== before) {
        node.textContent = after;
        changed = true;
      }
    }
  }
  return changed ? root.outerHTML : svg;
}
