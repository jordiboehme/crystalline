/**
 * What the rename field shows and what a label is written as, plus the two
 * tables the menus offer: the shapes and the colour presets.
 *
 * The delimiters and the `@{ shape }` names come from `model.ts`, the one
 * name table (R9); this file adds only the menu words. The ellipse `(-x-)`
 * and `icon` draw no box (R5), so neither is in `SHAPES`.
 */

import type { StatusColour } from "../../../markdown/directives";
import { entityText } from "../../../theme/mermaidText";
import { AT_SHAPE_NAMES, CLASSIC_SHAPES, type ShapeName } from "./model";

/** A colour preset's name: the same six the status labels use. */
export type ColourPreset = StatusColour;

/** What a word after `#` in the source stands for: `9829` is a number, `quot` a name. */
function sourceEntity(word: string): string | null {
  return entityText(/^\d+$/.test(word) ? `#${word}` : word);
}

/**
 * A label's text as the rename field shows it, which is what is drawn: no
 * quotes, no markdown wrapper, every `<br>` form as a line break and each
 * entity form as its character. One pass, so `#35;quot;` is the text `#quot;`.
 * An entity with a name `entityText` does not know stays as written. An
 * unquoted label loses its outer spaces, which mermaid does not keep either.
 */
export function decodeLabel(raw: string, quoted: boolean): string {
  let text = raw.trim();
  if (quoted) {
    const inner = raw.slice(1, -1);
    text =
      inner.startsWith("`") && inner.endsWith("`") && inner.length >= 2
        ? inner.slice(1, -1)
        : inner;
  }
  return text.replace(
    /<br\s*\/?>|#(\w+);|&(#\d+|\w+);/gi,
    (form, word: string | undefined, drawn: string | undefined) => {
      if (word !== undefined) {
        return sourceEntity(word) ?? form;
      }
      return drawn === undefined ? "\n" : (entityText(drawn) ?? form);
    },
  );
}

/**
 * What makes a label need quotes: mermaid's text delimiters and entity marks,
 * every character that is written as an entity form, outer space, outer `/` or
 * `-`, which would read as part of a shape's delimiter (`[/x`, `x/]`, `(-x`,
 * `x-)`), and `@` and `~~`, which mermaid's lexer reads as an arrow id or an
 * invisible arrow outside quotes.
 */
const NEEDS_QUOTES = /[[\]{}()|<>&;#%"@\\`\uFB02\xB6]|~~|^\s|\s$|^[-/]|[-/]$/;

/** One line break form, for a field that hands over CRLF. */
function oneBreak(text: string): string {
  return text.replace(/\r\n?/g, "\n");
}

/**
 * The entity form of each character mermaid would change on the way to an SVG
 * label, found in its source and pinned in a browser (`mermaid-labels.spec.ts`):
 * - `"` ends the string, a backtick at its start makes it a markdown string
 * - `<` and `>` are word breaks in `nonMarkdownToLines`, and `<...>` is markup
 *   to `sanitizeText`
 * - `\` before an `n` is a line break
 * - `#word;` and `&word;` are entities to `encodeEntities` and `sanitizeText`
 * - U+FB02 and U+00B6 start `decodeEntities`' two markers
 */
const ENTITY_FORMS: Readonly<Record<string, string>> = {
  '"': "#quot;",
  "<": "#lt;",
  ">": "#gt;",
  "\\": "#92;",
  "`": "#96;",
  "#": "#35;",
  "&": "#38;",
  "\uFB02": "#64258;",
  "\xB6": "#182;",
};

/** The characters of `ENTITY_FORMS` where they would be changed. */
const MANGLED =
  String.raw`["<>\\` + "`" + String.raw`ﬂ\xB6]|#(?=\w+;)|&(?=#?\w+;)`;

/**
 * What markdown takes out of a label on top of `MANGLED`: `markedjs` reads links,
 * autolinks and strikethrough, and mermaid's `markdownToLines` draws none of them,
 * so the text would vanish. Blocks (`- x`, `# x`) come through as written.
 */
const LINK_MARKS = String.raw`|[[\]~:@]|(?<=www)\.`;

/** A plain text's marks in a label that mermaid reads as markdown (the `@{ }` form). */
const EMPHASIS_MARKS = String.raw`|[*_]`;

const IN_STRING = new RegExp(MANGLED, "g");
const IN_MARKDOWN = new RegExp(MANGLED + LINK_MARKS, "gi");
const AS_PLAIN_MARKDOWN = new RegExp(
  MANGLED + LINK_MARKS + EMPHASIS_MARKS,
  "gi",
);

/** The text as it goes between quotes: `marks` as entity forms, `\n` as `<br>`. */
function quotedText(text: string, marks: RegExp = IN_STRING): string {
  return text
    .replace(marks, (c) => ENTITY_FORMS[c] ?? `#${String(c.codePointAt(0))};`)
    .replace(/\n/g, "<br>");
}

/**
 * The text a label can draw: SVG text keeps no outer space and no run of
 * spaces, so what `rename` writes and compares is this. Blank lines inside stay,
 * they draw as an empty row.
 */
export function tidyLabel(text: string): string {
  return oneBreak(text)
    .split("\n")
    .map((line) => line.replace(/[ \t]+/g, " ").trim())
    .join("\n")
    .replace(/^\n+|\n+$/g, "");
}

/**
 * The label to write for `text`: `\n` as `<br>`, in quotes when it needs them
 * or when `keepQuoted`. Inside quotes every character mermaid would change is
 * its entity form, and `drawnLabels` (`theme/mermaidText.ts`) draws those as the
 * character, so what was typed is what is drawn.
 */
export function encodeLabel(text: string, keepQuoted: boolean): string {
  const one = oneBreak(text);
  return keepQuoted || NEEDS_QUOTES.test(one)
    ? `"${quotedText(one)}"`
    : one.replace(/\n/g, "<br>");
}

/** The label to write for `text` inside a markdown string's wrapper. */
export function encodeMarkdownLabel(text: string): string {
  return `"\`${quotedText(oneBreak(text), IN_MARKDOWN)}\`"`;
}

/**
 * The `@{ label: }` value for `text`. mermaid reads this form as markdown whether
 * or not anybody wrote markdown, so every mark is an entity form here too. A value
 * stays bare only when it is the parser's bare value, the one YAML cannot read as
 * something else, with no mark in it and when it was not quoted (`keepQuoted`).
 */
export function encodeAtLabel(text: string, keepQuoted: boolean): string {
  const one = oneBreak(text);
  const written = quotedText(one, AS_PLAIN_MARKDOWN);
  const bare =
    !keepQuoted &&
    written === one &&
    /^[A-Za-z][A-Za-z0-9 _.-]*$/.test(one) &&
    !/\s$|^(?:true|false|null)$/i.test(one);
  return bare ? one : `"${written}"`;
}

export const COLOUR_PRESETS: Record<
  ColourPreset,
  { label: string; fill: string; stroke: string; color: string }
> = {
  grey: { label: "Grey", fill: "#475569", stroke: "#1e293b", color: "#ffffff" },
  blue: { label: "Blue", fill: "#1d4ed8", stroke: "#1e3a8a", color: "#ffffff" },
  green: {
    label: "Green",
    fill: "#15803d",
    stroke: "#14532d",
    color: "#ffffff",
  },
  yellow: {
    label: "Yellow",
    fill: "#a16207",
    stroke: "#713f12",
    color: "#ffffff",
  },
  red: { label: "Red", fill: "#b91c1c", stroke: "#7f1d1d", color: "#ffffff" },
  purple: {
    label: "Purple",
    fill: "#7e22ce",
    stroke: "#581c87",
    color: "#ffffff",
  },
};

const SHAPE_LABELS: Record<ShapeName, string> = {
  rect: "Rectangle",
  rounded: "Rounded",
  stadium: "Stadium",
  subroutine: "Subroutine",
  cylinder: "Database",
  circle: "Circle",
  doubleCircle: "Double circle",
  flag: "Flag",
  diamond: "Decision",
  hexagon: "Hexagon",
  leanRight: "Slanted right",
  leanLeft: "Slanted left",
  trapezoid: "Trapezoid",
  trapezoidAlt: "Upside-down trapezoid",
};

/**
 * The short `@{ shape }` name to write for a shape: the first of its names in
 * `AT_SHAPE_NAMES`, which lists each shape's short name before its aliases.
 */
function atName(name: ShapeName): string {
  const entry = Object.entries(AT_SHAPE_NAMES).find(([, n]) => n === name);
  if (entry === undefined) {
    throw new Error(`no @{ shape } name for ${name}`);
  }
  return entry[0];
}

/** The fourteen shapes in the menu's order. */
export const SHAPES: readonly {
  name: ShapeName;
  label: string;
  open: string;
  close: string;
  at: string;
}[] = CLASSIC_SHAPES.map((shape) => ({
  name: shape.name,
  label: SHAPE_LABELS[shape.name],
  open: shape.open,
  close: shape.close,
  at: atName(shape.name),
}));
