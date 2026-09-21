/**
 * The one thing a widened diagram needs: its own width back.
 *
 * Mermaid renders with `useMaxWidth` on by default, so the root arrives as
 * `width="100%"` with an inline `max-width` and the browser scales the whole
 * drawing down to whatever column it lands in. That is the fitted drawing
 * every diagram shows first. A reader who asks for full width wants the room
 * there is, and never less than the drawing's own size; this module rewrites
 * the root tag to say exactly that.
 *
 * Pure string work on purpose. It runs on markup mermaid just produced, before
 * that markup is ever in the document, so there is no element to measure and
 * nothing to query; and a pure function is a thing tests can pin exactly,
 * which layout in jsdom is not.
 */

/**
 * Give a diagram full width: the root takes `width="100%"` with mermaid's
 * clamp gone, which lets the viewBox scale a small diagram UP to fill a wide
 * column, and an inline `min-width` of the drawing's own width, which stops a
 * column narrower than the drawing from squeezing it.
 *
 * A diagram whose natural width cannot be read gets `width="100%"` and no
 * floor, because there is no number to hold it up with. Running this twice
 * changes nothing, so a second press of the same button never stacks a second
 * floor.
 */
export function unclampDiagram(svg: string): string {
  const root = findRootTag(svg);
  if (root === null) {
    return svg;
  }
  const tag = svg.slice(root.start, root.end);
  const width = naturalWidth(tag);
  const widened = withWidth(tag, "100%");
  const unclamped =
    width === null ? withoutClamp(widened) : withFloor(widened, width);
  return svg.slice(0, root.start) + unclamped + svg.slice(root.end);
}

/**
 * The span of the opening `<svg>` tag, quotes respected so an attribute value
 * holding a `>` does not end the tag early. Only the root is ever rewritten:
 * mermaid nests `<svg>` elements for images and icons, and a `<style>` block
 * inside the diagram may say `max-width` about something else entirely.
 */
function findRootTag(markup: string): { start: number; end: number } | null {
  const open = /<svg(?=[\s/>])/i.exec(markup);
  if (open === null) {
    return null;
  }
  let quote: string | null = null;
  for (let i = open.index; i < markup.length; i += 1) {
    const char = markup[i];
    if (quote !== null) {
      if (char === quote) {
        quote = null;
      }
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }
    if (char === ">") {
      return { start: open.index, end: i + 1 };
    }
  }
  return null;
}

/**
 * The drawing's own width in pixels: the viewBox first, since that is what
 * mermaid always writes and what actually describes the geometry, then a
 * `width` attribute that reads as pixels. Mermaid's own `width="100%"` reads
 * as nothing at all, which is correct - a percentage says how the diagram is
 * scaled, never how wide it is - and a diagram whose width cannot be read is
 * left exactly as it is.
 */
function naturalWidth(tag: string): number | null {
  const viewBox = attribute(tag, "viewBox");
  if (viewBox !== null) {
    const parts = viewBox.trim().split(/[\s,]+/);
    // "minX minY width height", so the third number is the one that matters.
    const declared = parts.length === 4 ? parts[2] : undefined;
    if (declared !== undefined) {
      const width = pixels(declared);
      if (width !== null) {
        return width;
      }
    }
  }
  const declared = attribute(tag, "width");
  return declared === null ? null : pixels(declared);
}

function pixels(value: string): number | null {
  const match = /^\s*(\d+(?:\.\d+)?)(?:px)?\s*$/.exec(value);
  if (match === null) {
    return null;
  }
  const width = Number(match[1]);
  return width > 0 ? width : null;
}

function attribute(tag: string, name: string): string | null {
  const match = new RegExp(
    `\\s${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)')`,
    "i",
  ).exec(tag);
  if (match === null) {
    return null;
  }
  return match[1] ?? match[2] ?? "";
}

/** Replace the width the renderer wrote, or write one where there was none. */
function withWidth(tag: string, value: string): string {
  const existing = /(\swidth\s*=\s*)(?:"[^"]*"|'[^']*')/i;
  if (existing.test(tag)) {
    return tag.replace(existing, `$1"${value}"`);
  }
  return tag.replace(/^<svg/i, `<svg width="${value}"`);
}

/** Drop the inline `max-width` and keep whatever else the style said. */
function withoutClamp(tag: string): string {
  const style = /(\sstyle\s*=\s*)(?:"([^"]*)"|'([^']*)')/i;
  const match = style.exec(tag);
  if (match === null) {
    return tag;
  }
  const declarations = (match[2] ?? match[3] ?? "")
    .split(";")
    .map((declaration) => declaration.trim())
    .filter(
      (declaration) =>
        declaration !== "" && !/^max-width\s*:/i.test(declaration),
    );
  const before = tag.slice(0, match.index);
  const after = tag.slice(match.index + match[0].length);
  if (declarations.length === 0) {
    return before + after;
  }
  return `${before}${match[1]}"${declarations.join("; ")};"${after}`;
}

/**
 * Drop the clamp and put a floor in its place: the drawing's own width as an
 * inline `min-width`, replacing any that was already there so a second pass
 * leaves the markup exactly as the first one did. The floor goes last, which
 * is what makes that true of the whole declaration string and not only of the
 * value.
 */
function withFloor(tag: string, width: number): string {
  const style = /(\sstyle\s*=\s*)(?:"([^"]*)"|'([^']*)')/i;
  const match = style.exec(tag);
  const declarations = (match === null ? "" : (match[2] ?? match[3] ?? ""))
    .split(";")
    .map((declaration) => declaration.trim())
    .filter(
      (declaration) =>
        declaration !== "" &&
        !/^max-width\s*:/i.test(declaration) &&
        !/^min-width\s*:/i.test(declaration),
    );
  declarations.push(`min-width: ${width}px`);
  const written = `${declarations.join("; ")};`;
  if (match === null) {
    return tag.replace(/^<svg/i, `<svg style="${written}"`);
  }
  const before = tag.slice(0, match.index);
  const after = tag.slice(match.index + match[0].length);
  return `${before}${match[1]}"${written}"${after}`;
}
