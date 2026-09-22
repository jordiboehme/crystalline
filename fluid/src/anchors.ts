/**
 * Section anchors: the names a heading is reachable by.
 *
 * A web URL for an engram may end in `#auth`, and the heading that fragment
 * names has to be the same heading whoever wrote the URL meant. Nothing
 * server-side knows about anchors - the fragment never leaves the browser - so
 * the rule itself is the agreement: an agent derives the slug from the heading
 * text it already has, this module assigns the very same slug as an id, and the
 * two meet at a heading rather than at a shared list of them.
 *
 * The rule is kept here rather than in the renderer because it is a rule about
 * text, not about drawing: the shared corpus at `tests/fixtures/web-url` pins
 * it, and `anchors.test.ts` is where the two sides are held to one answer.
 */

/**
 * Everything a slug drops: whatever is not a letter, a combining mark, a
 * decimal digit, whitespace, a hyphen or an underscore.
 *
 * Unicode aware on purpose. A heading in German or Greek keeps its letters
 * rather than being transliterated into something its writer would not
 * recognize, and a combining mark stays with the letter it sits on.
 */
const KEEP = /[^\p{L}\p{M}\p{Nd}\s_-]/gu;

/**
 * One heading's text as a slug.
 *
 * Lowercase, keep letters, marks, digits, hyphens and underscores, runs of
 * whitespace become one hyphen. A heading whose text is all punctuation leaves
 * nothing to name it by, so it is called `section`.
 */
export function headingSlug(text: string): string {
  const slug = text.toLowerCase().replace(KEEP, "").trim().replace(/\s+/g, "-");
  return slug === "" ? "section" : slug;
}

/**
 * The assigner one document runs on: the same slug rule, plus the memory of
 * what this document has already used.
 *
 * Two headings called "API" cannot both be `#api`, so the first keeps it and
 * the later ones are numbered in document order. The count is per base rather
 * than global, and a numbered candidate that is itself already taken - a
 * document holding a heading literally called "API 1" - keeps counting until
 * it lands on a free name, so an id is never assigned twice.
 */
export function slugAssigner(): (text: string) => string {
  const taken = new Set<string>();
  const counts = new Map<string, number>();
  return (text) => {
    const base = headingSlug(text);
    let candidate = base;
    let n = counts.get(base) ?? 0;
    while (taken.has(candidate)) {
      n += 1;
      candidate = `${base}-${n}`;
    }
    counts.set(base, n);
    taken.add(candidate);
    return candidate;
  };
}

/**
 * A hast node, narrowed to what this module reads off one.
 *
 * Declared here for the same reason `MarkdownBody.tsx` declares its own: those
 * types are a transitive dependency of react-markdown rather than one this
 * package declares.
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

/** Whether this element is a heading of any level. */
const HEADING = /^h[1-6]$/;

/**
 * The rehype plugin that gives every heading in a document its id.
 *
 * A plugin rather than a pass in the component map: a component is handed one
 * heading at a time and cannot know which ones came before it, and the
 * numbering of a repeat is exactly that knowledge. One assigner per run, so
 * every document starts its counting over.
 *
 * A heading that already carries an id keeps it: the page's own title heading
 * is named by the screen that draws it, and nothing here renames what somebody
 * else has already named.
 */
export function headingIds() {
  return function plugin() {
    return function transform(tree: unknown) {
      walk(tree as HastNode, slugAssigner());
    };
  };
}

/** Walk one node, naming every heading under it in document order. */
function walk(node: HastNode, assign: (text: string) => string): void {
  if (
    node.tagName !== undefined &&
    HEADING.test(node.tagName) &&
    typeof node.properties?.id !== "string"
  ) {
    node.properties = { ...node.properties, id: assign(textOf(node)) };
    return;
  }
  for (const child of node.children ?? []) {
    walk(child, assign);
  }
}

/**
 * The URL of one section: the page's own URL and the fragment, written as they
 * are.
 *
 * No encoding here, deliberately. The slug keeps the letters the heading was
 * written with, a browser percent-encodes the fragment on the wire by itself,
 * and what a person copies stays readable - `#über-uns` rather than a run of
 * hex. Fluid decodes `location.hash` before it looks the element up, so the
 * round trip closes either way.
 */
export function sectionUrl(pageUrl: string, slug: string): string {
  return `${pageUrl}#${slug}`;
}

/** The class the arriving section wears while it is being pointed out. */
export const SECTION_FLASH_CLASS = "section-flash";

/** How long that highlight lasts, matching the animation in index.css. */
export const SECTION_FLASH_MS = 2500;
