/**
 * The flowchart model: what `parseFlowchart` reads out of a fence body, with
 * the body span of every part an op may edit. Every span is body-relative
 * (`Span` from `../edits`), so an op compiles to `TextEdit`s on exactly the
 * bytes it means and leaves every other byte as written.
 *
 * The model follows mermaid 12.2.0's own reading of the source
 * (research/2026-11-05-mermaid-12.2-flowchart-ids.md): edge ids and order,
 * subgraph membership and "the last label wins" are computed the way its
 * FlowDB computes them, and `agreesFlow` (`../verdict`) checks the result
 * against mermaid's db before a fence is drawn.
 */

import type { Span } from "../edits";
import type { Scan } from "../scan";

/** The fourteen classic shapes mermaid 12.2.0 draws (the ellipse is not one, R5). */
export type ShapeName =
  | "rect"
  | "rounded"
  | "stadium"
  | "subroutine"
  | "cylinder"
  | "circle"
  | "doubleCircle"
  | "flag"
  | "diamond"
  | "hexagon"
  | "leanRight"
  | "leanLeft"
  | "trapezoid"
  | "trapezoidAlt";

/**
 * A box's shape and label as written.
 *
 * - `form: "delimiters"`: `open` and `close` are the delimiters (`[`, `((`,
 *   `[/`, ...) and `label` is everything between them, quotes and markdown
 *   backticks included, so `open.to === label.from` and
 *   `label.to === close.from`.
 * - `form: "at"`: `A@{ shape: diam, label: "x" }`. `open` is `@{`, `close`
 *   the `}`, `label` the label value (quotes included when `quoted`) and
 *   `atShape` the shape value. `name` is null for a shape name outside the
 *   fourteen (`doc`, ...).
 */
export interface Shape {
  form: "delimiters" | "at";
  name: ShapeName | null;
  open: Span;
  close: Span;
  label: Span;
  /** The label is a `"..."` string (for markdown, `"` + backtick ... backtick + `"`). */
  quoted: boolean;
  /** The label is a markdown string, `"` + backtick ... backtick + `"`. */
  markdown: boolean;
  atShape: Span | null;
}

/** One mention of a box id in a chain statement. */
export interface NodeRef {
  id: string;
  idSpan: Span;
  shape: Shape | null;
  /** The class name after `:::` (without the colons), when there is one. */
  classSpan: Span | null;
  /** The whole ref: id, shape and `:::class`. */
  span: Span;
  /** The statement's index among all statements of the body, the header's first. */
  statement: number;
  /** The body line the ref is on. */
  line: number;
}

export type Head = "none" | "arrow" | "circle" | "cross";

/**
 * One link token as written.
 *
 * - `form: "plain"`: `span` is the token (`-->`), without the whitespace
 *   around it.
 * - `form: "pipe"`: as plain, plus `label`, everything between the two `|`
 *   (quotes included when `labelQuoted`).
 * - `form: "text"`: `-- text -->`. `opener` (`--`) and `closer` (`-->`) are
 *   the two tokens without their whitespace, `span` runs from the opener's
 *   start to the closer's end and `label` is the text between them, trimmed
 *   (the whitespace belongs to the tokens).
 *
 * `idSpan` is the explicit id of `A e1@--> B` (`e1`, without the `@`).
 * `length` is as written; mermaid caps it at 10.
 */
export interface Arrow {
  span: Span;
  line: "solid" | "dotted" | "thick" | "invisible";
  start: Head;
  end: Head;
  length: number;
  form: "plain" | "pipe" | "text";
  label: Span | null;
  labelQuoted: boolean;
  opener: Span | null;
  closer: Span | null;
  idSpan: Span | null;
}

/** `A & B`: the refs on one side of a link, and the span from the first to the last. */
export interface Group {
  refs: NodeRef[];
  span: Span;
}

/**
 * The link between `groups[i]` and `groups[i + 1]`. `gap` runs from the end
 * of the last ref of `groups[i]` to the start of the first ref of
 * `groups[i + 1]`, whitespace, explicit id and label included. `edges` are
 * the indices into `FlowModel.edges` the link makes.
 */
export interface Link {
  gap: Span;
  arrow: Arrow;
  edges: number[];
}

/** A statement of boxes and links: `A --> B & C --> D`, or a lone `A[x]`. */
export interface Chain {
  statement: number;
  line: number;
  span: Span;
  groups: Group[];
  links: Link[];
}

/**
 * One edge in mermaid's order (`db.getEdges()`, which `linkStyle` indexes).
 * `occurrence` counts the earlier edges with the same start and end.
 */
export interface FlowEdge {
  index: number;
  start: string;
  end: string;
  mermaidId: string;
  occurrence: number;
  chain: number;
  link: number;
}

/**
 * One box. `refs` are its mentions in chain statements, in source order
 * (empty for a box that only a `style` line names). `defining` is the last
 * ref with a shape, the one whose label mermaid shows. `subgraph` is the
 * subgraph mermaid's db lists it in, or null.
 */
export interface FlowNode {
  id: string;
  refs: NodeRef[];
  defining: NodeRef | null;
  subgraph: string | null;
}

/**
 * A `style`, `classDef`, `class`, `click`, `linkStyle` or `direction`
 * statement.
 *
 * - `targets`: the box ids of `style`, `class` and `click`, the class names
 *   of `classDef`; empty for `linkStyle` and `direction`.
 * - `props`: the `key:value` entries of `style`, `classDef` and `linkStyle`
 *   (`key_` the key, `value` the value, `entry` both); for `direction` one
 *   entry with key `direction` whose value is the direction.
 * - `indices`: the numbers of `linkStyle`, each span over its digits only;
 *   empty for `linkStyle default`.
 * - `className`: the class a `class` line gives, null for every other kind.
 */
export interface DirectiveLine {
  kind: "style" | "classDef" | "class" | "click" | "linkStyle" | "direction";
  line: number;
  span: Span;
  targets: { id: string; span: Span }[];
  props: { key: string; key_: Span; value: Span; entry: Span }[];
  indices: { n: number; span: Span }[];
  className: { name: string; span: Span } | null;
}

/**
 * A `subgraph ... end` block. `id` is mermaid's (`subGraph<N>` for a block
 * without one, numbered in the order the blocks close). `members` is the
 * list mermaid's db gives it, as a set: the ids its own statements name
 * plus its child subgraphs, without ids an earlier closed subgraph already
 * holds.
 */
export interface Subgraph {
  id: string;
  openLine: number;
  endLine: number;
  parent: string | null;
  members: string[];
}

export interface FlowModel {
  kind: "flowchart";
  keyword: string;
  /** The header's direction as written (`TD`, `LR`, `>`, ...), null when it has none. */
  direction: { value: string; span: Span } | null;
  headerLine: number;
  nodes: Map<string, FlowNode>;
  /** Node ids in the order mermaid's db first meets them. */
  nodeOrder: string[];
  edges: FlowEdge[];
  chains: Chain[];
  directives: DirectiveLine[];
  subgraphs: Subgraph[];
  scan: Scan;
  /** The body the spans point into. */
  body: string;
  understood: boolean;
  reasons: string[];
}

/** An edge named the way an op names it, stable across unrelated edits. */
export interface EdgeKey {
  start: string;
  end: string;
  occurrence: number;
}

/** One classic shape: its delimiters and the type mermaid's db gives it. */
export interface ClassicShape {
  name: ShapeName;
  open: string;
  close: string;
  dbType: string;
}

/** The fourteen classic delimiter pairs and mermaid 12.2.0's db type for each. */
export const CLASSIC_SHAPES: readonly ClassicShape[] = [
  { name: "rect", open: "[", close: "]", dbType: "square" },
  { name: "rounded", open: "(", close: ")", dbType: "round" },
  { name: "stadium", open: "([", close: "])", dbType: "stadium" },
  { name: "subroutine", open: "[[", close: "]]", dbType: "subroutine" },
  { name: "cylinder", open: "[(", close: ")]", dbType: "cylinder" },
  { name: "circle", open: "((", close: "))", dbType: "circle" },
  {
    name: "doubleCircle",
    open: "(((",
    close: ")))",
    dbType: "doublecircle",
  },
  { name: "flag", open: ">", close: "]", dbType: "odd" },
  { name: "diamond", open: "{", close: "}", dbType: "diamond" },
  { name: "hexagon", open: "{{", close: "}}", dbType: "hexagon" },
  { name: "leanRight", open: "[/", close: "/]", dbType: "lean_right" },
  { name: "leanLeft", open: "[\\", close: "\\]", dbType: "lean_left" },
  { name: "trapezoid", open: "[/", close: "\\]", dbType: "trapezoid" },
  {
    name: "trapezoidAlt",
    open: "[\\",
    close: "/]",
    dbType: "inv_trapezoid",
  },
];

/**
 * The `@{ shape: ... }` names mermaid 12.2.0 accepts for the fourteen
 * shapes, each to its ShapeName (research note, shape table). The db type
 * of an `@{ shape }` box is the name as written (R9).
 */
export const AT_SHAPE_NAMES: Readonly<Record<string, ShapeName>> = {
  rect: "rect",
  proc: "rect",
  process: "rect",
  rectangle: "rect",
  rounded: "rounded",
  event: "rounded",
  stadium: "stadium",
  terminal: "stadium",
  pill: "stadium",
  "fr-rect": "subroutine",
  subprocess: "subroutine",
  subproc: "subroutine",
  "framed-rectangle": "subroutine",
  subroutine: "subroutine",
  cyl: "cylinder",
  db: "cylinder",
  database: "cylinder",
  cylinder: "cylinder",
  circle: "circle",
  circ: "circle",
  "dbl-circ": "doubleCircle",
  "double-circle": "doubleCircle",
  odd: "flag",
  diam: "diamond",
  decision: "diamond",
  diamond: "diamond",
  question: "diamond",
  hex: "hexagon",
  hexagon: "hexagon",
  prepare: "hexagon",
  "lean-r": "leanRight",
  "lean-right": "leanRight",
  "in-out": "leanRight",
  "lean-l": "leanLeft",
  "lean-left": "leanLeft",
  "out-in": "leanLeft",
  "trap-b": "trapezoid",
  priority: "trapezoid",
  "trapezoid-bottom": "trapezoid",
  trapezoid: "trapezoid",
  "trap-t": "trapezoidAlt",
  manual: "trapezoidAlt",
  "trapezoid-top": "trapezoidAlt",
  "inv-trapezoid": "trapezoidAlt",
};
