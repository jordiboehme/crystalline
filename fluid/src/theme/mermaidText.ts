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

/**
 * `svg` with the entity forms in its drawn text decoded, one pass per text
 * node (see the file header for what counts as drawn text). Styles, attributes,
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
