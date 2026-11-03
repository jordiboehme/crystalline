/**
 * Clipboard HTML in an inert document, and what the paste handler asks of it
 * before the converter has loaded.
 *
 * No turndown here: the paste handler imports this module eagerly, and the
 * converter (`htmlToMarkdown.ts`) stays a lazy chunk.
 */

/** Elements removed with everything inside them before any conversion. */
const REMOVED = [
  "script",
  "style",
  "iframe",
  "frame",
  "frameset",
  "object",
  "embed",
  "applet",
  "link",
  "meta",
  "base",
  "noscript",
  "template",
  "title",
];

/**
 * The clipboard HTML parsed into an inert document, with active elements and
 * every `on*` attribute removed. The cleaning is checked on its
 * own (an event attribute never shows in the Markdown either way).
 */
export function cleanClipboardHtml(html: string): Document {
  const doc = new DOMParser().parseFromString(html, "text/html");
  for (const el of Array.from(doc.querySelectorAll(REMOVED.join(",")))) {
    el.remove();
  }
  for (const el of Array.from(doc.querySelectorAll("*"))) {
    for (const name of el.getAttributeNames()) {
      if (name.toLowerCase().startsWith("on")) {
        el.removeAttribute(name);
      }
    }
  }
  return doc;
}

/**
 * Whether the HTML holds any text once it is cleaned: what tells a copy of
 * text (with a picture of it beside) from a copy of an image.
 */
export function carriesText(html: string): boolean {
  return textOf(html) !== "";
}

/**
 * The text of the cleaned HTML, trimmed: what a plain paste falls back to
 * when the clipboard brought HTML but no plain text.
 */
export function textOf(html: string): string {
  return (cleanClipboardHtml(html).body.textContent ?? "").trim();
}

/**
 * `text` with every HTML character reference decoded, again and again until
 * nothing changes: what a Markdown renderer may decode in a link destination
 * (`&#106;`, `&colon;`, `&Tab;`) is decoded here too, however many times it
 * was encoded. Decoded through an inert document; a `<` is kept from opening
 * a tag.
 */
export function decodeEntities(text: string): string {
  let current = text;
  for (let round = 0; round < 8; round += 1) {
    const doc = new DOMParser().parseFromString(
      `<body>${current.replace(/</g, "&lt;")}`,
      "text/html",
    );
    const next = doc.body.textContent ?? "";
    if (next === current) {
      break;
    }
    current = next;
  }
  return current;
}

/**
 * Whether the HTML is the copy Fluid wrote itself: its root element carries
 * the marker, after whatever a browser wraps around a fragment. Its
 * `text/plain` is the Markdown the person copied and a paste takes that as it
 * is. The marker earns no trust beyond that: the plain text is the same text
 * the person sees, whoever wrote the HTML.
 */
export function isOwnCopy(html: string): boolean {
  if (!html.includes("data-crystalline-copy")) {
    return false;
  }
  const first = new DOMParser().parseFromString(html, "text/html").body
    .firstElementChild;
  return first?.getAttribute("data-crystalline-copy") === "1";
}
