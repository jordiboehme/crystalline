/**
 * What the rename field shows and what a label is written as, plus the two
 * tables the menus offer: the shapes and the colour presets.
 *
 * The delimiters and the `@{ shape }` names come from `model.ts`, the one
 * name table (R9); this file adds only the menu words. The ellipse `(-x-)`
 * and `icon` draw no box (R5), so neither is in `SHAPES`.
 */

import type { StatusColour } from "../../../markdown/directives";
import { AT_SHAPE_NAMES, CLASSIC_SHAPES, type ShapeName } from "./model";

/** A colour preset's name: the same six the status labels use. */
export type ColourPreset = StatusColour;

/**
 * A label's text as the rename field shows it: no quotes, no markdown wrapper,
 * `#quot;` as `"`, `#35;` as `#` and every `<br>` form as a line break. One
 * pass, so `#35;quot;` is the text `#quot;`. An unquoted label loses its outer
 * spaces, which mermaid does not keep either.
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
  return text
    .replace(/#(quot|35);/g, (_, name: string) => (name === "quot" ? '"' : "#"))
    .replace(/<br\s*\/?>/gi, "\n");
}

/**
 * What makes a label need quotes: mermaid's text delimiters and entity marks,
 * outer space, outer `/`, `\` or `-`, which would read as part of a shape's
 * delimiter (`[/x`, `x\]`, `(-x`, `x-)`), and `@` and `~~`, which mermaid's
 * lexer reads as an arrow id or an invisible arrow outside quotes.
 */
const NEEDS_QUOTES = /[[\]{}()|<>&;#%"@]|~~|^\s|\s$|^[-/\\]|[-/\\]$/;

/** One line break form, for a field that hands over CRLF. */
function oneBreak(text: string): string {
  return text.replace(/\r\n?/g, "\n");
}

/** The text as it goes between quotes. */
function quotedText(text: string): string {
  return text
    .replace(/#(?=[A-Za-z0-9]+;)/g, "#35;")
    .replace(/"/g, "#quot;")
    .replace(/\n/g, "<br>");
}

/**
 * The label to write for `text`: `\n` as `<br>`, in quotes when it needs them
 * or when `keepQuoted`. Inside quotes `"` is `#quot;` and a `#` that would read
 * as an entity is `#35;`.
 */
export function encodeLabel(text: string, keepQuoted: boolean): string {
  const one = oneBreak(text);
  return keepQuoted || NEEDS_QUOTES.test(one)
    ? `"${quotedText(one)}"`
    : one.replace(/\n/g, "<br>");
}

/** The label to write for `text` inside a markdown string's wrapper. */
export function encodeMarkdownLabel(text: string): string {
  return `"\`${quotedText(oneBreak(text))}\`"`;
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
