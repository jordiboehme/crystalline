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
 * The result goes in through text nodes and comes out through the serializer,
 * which escapes it, so a label reading `<script>` stays the text `<script>`.
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

/**
 * `svg` with the entity forms in its drawn text decoded, one pass per text
 * node. Styles, attributes, titles and foreign objects stay as they are. An svg
 * with nothing to decode comes back as the same string.
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
    const walker = doc.createTreeWalker(text, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const before = node.textContent ?? "";
      const after = before.replace(
        /&(#\d+|\w+);/g,
        (form, name: string) => entityText(name) ?? form,
      );
      if (after !== before) {
        node.textContent = after;
        changed = true;
      }
    }
  }
  return changed ? root.outerHTML : svg;
}
