/**
 * The flowchart parser: reads a fence body into a `FlowModel` that keeps the
 * source span of every id, shape, label, arrow and directive, so the ops of
 * Tasks 4 to 6 can change only the bytes they mean.
 *
 * It works per statement span from `scanFence` and never runs a regex over
 * the whole body for structure. Inside a statement it reads tokens the way
 * mermaid 12.2.0's flowchart lexer does (its rules are copied where they
 * decide where a token ends: the id rule with its conditional `-`, the link
 * rules with their surrounding whitespace, `LINK_ID`), and it computes edge
 * ids, edge order, subgraph membership and the defining label the way
 * mermaid's FlowDB does. See research/2026-11-05-mermaid-12.2-flowchart-ids.md.
 *
 * It knows a deliberate subset of the grammar. Anything outside it leaves
 * `understood` false with a reason, and the fence keeps its text: a line it
 * cannot classify, a label or `@{ }` spread over lines, a statement it
 * cannot split into refs and links, a reserved word as an id, an unbalanced
 * `subgraph`/`end`, the ellipse `(-x-)` (R5), a direction line other than a
 * plain `direction XX` inside a subgraph (R7), a bare or trailing `%%` (R8),
 * a `linkStyle` index past the edges so far (R9), `flowchart-elk` (R30), a
 * body or an edge count past mermaid's limits, two edges with one id, and
 * what mermaid accepts but cannot draw (`@{ shape: icon }`).
 *
 * It never throws: a body it trips over comes back not understood.
 */

import type { Span } from "../edits";
import {
  type ScannedLine,
  hasStrayComment,
  isDirectionLine,
  scanFence,
} from "../scan";
import {
  AT_SHAPE_NAMES,
  type Arrow,
  type Chain,
  type DirectiveLine,
  type EdgeKey,
  type FlowEdge,
  type FlowModel,
  type FlowNode,
  type Group,
  type Head,
  type Link,
  type NodeRef,
  type Shape,
  type ShapeName,
  type Subgraph,
} from "./model";

/** The flowchart keywords this parser reads (R30 leaves `flowchart-elk` out). */
const KEYWORDS = new Set(["flowchart", "graph"]);

/**
 * mermaid's `maxTextSize`: `render` replaces a longer text with its "Maximum
 * text size" diagram, while the db read of the verdict does not notice.
 */
export const MAX_TEXT_SIZE = 50_000;
/** mermaid's `maxEdges`: the db throws on one edge more. */
export const MAX_EDGES = 500;

/**
 * `@{ shape }` names mermaid accepts that draw no `g.node`, so a click could
 * not map back to the box. Checked in Chromium over all 140 documented names
 * and the lowercase undocumented ones (`state`, `choice`, `note`, `composite`,
 * `anchor`, `doublecircle` draw one; `image` is refused by the db).
 */
const SHAPES_WITHOUT_BOX = new Set(["icon"]);

/**
 * mermaid's NODE_STRING (rule 106), plus any other non-space character
 * outside ASCII for UNICODE_TEXT (rule 108), so an id ends where mermaid's
 * does; `PLAIN_ID` then decides whether it is one this parser keeps.
 */
const ID_TOKEN =
  /(?:[A-Za-z0-9!"#$%&'*+.`?\\_/]|-(?=[^>\-.])|[^\p{ASCII}\s])+/uy;
/** The ids this parser accepts: what people write, nothing mermaid reads oddly. */
const PLAIN_ID = /^[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+)*$/;
/** Lexer rules that come before NODE_STRING and take a word as a keyword. */
const RESERVED =
  /^(?:style|linkStyle|interpolate|classDef|class|flowchart-elk|swimlane-beta|graph|flowchart|subgraph|end|_self|_blank|_parent|_top|accTitle|accDescr)\b|^(?:call|href|click)(?:\s|$)/;
/** The link rules in lexer order (70, 71, 73, 74, 76, 77, 79). */
interface LinkRule {
  re: RegExp;
  /** For a START_LINK rule: the closing rule and the label characters it allows. */
  text?: { close: RegExp; char: (text: string, i: number) => boolean };
}
const NORMAL = /\s*[xo<]?--+[-xo>]\s*/y;
const THICK = /\s*[xo<]?==+[=xo>]\s*/y;
const DOTTED = /\s*[xo<]?-?\.+-[xo>]?\s*/y;
const LINK_RULES: LinkRule[] = [
  { re: NORMAL },
  {
    re: /\s*[xo<]?--\s*/y,
    text: {
      close: NORMAL,
      char: (t, i) => t[i] !== "-" || t[i + 1] !== "-",
    },
  },
  { re: THICK },
  {
    re: /\s*[xo<]?==\s*/y,
    text: { close: THICK, char: (t, i) => t[i] !== "=" },
  },
  { re: DOTTED },
  {
    re: /\s*[xo<]?-\.\s*/y,
    text: { close: DOTTED, char: (t, i) => t[i] !== "." },
  },
  { re: /\s*~~[~]+\s*/y },
];

/** Characters the lexer turns into tokens inside a label: never plain label text. */
const LABEL_STOP = /[[\](){}|"]/;

/** The delimiter openers, longest first, and the closers each allows. */
const OPENERS: {
  open: string;
  closers: { close: string; name: ShapeName }[];
  trap?: true;
}[] = [
  { open: "(((", closers: [{ close: ")))", name: "doubleCircle" }] },
  { open: "((", closers: [{ close: "))", name: "circle" }] },
  { open: "([", closers: [{ close: "])", name: "stadium" }] },
  { open: "(", closers: [{ close: ")", name: "rounded" }] },
  { open: "[[", closers: [{ close: "]]", name: "subroutine" }] },
  { open: "[(", closers: [{ close: ")]", name: "cylinder" }] },
  {
    open: "[/",
    closers: [
      { close: "/]", name: "leanRight" },
      { close: "\\]", name: "trapezoid" },
    ],
    trap: true,
  },
  {
    open: "[\\",
    closers: [
      { close: "\\]", name: "leanLeft" },
      { close: "/]", name: "trapezoidAlt" },
    ],
    trap: true,
  },
  { open: "[", closers: [{ close: "]", name: "rect" }] },
  { open: "{{", closers: [{ close: "}}", name: "hexagon" }] },
  { open: "{", closers: [{ close: "}", name: "diamond" }] },
  { open: ">", closers: [{ close: "]", name: "flag" }] },
];

/** A failure inside one statement: the message for `reasons`. */
class Refusal extends Error {}

function refuse(message: string): never {
  throw new Refusal(message);
}

interface OpenSubgraph {
  /** null: mermaid numbers it `subGraph<N>` when it closes. */
  named: string | null;
  title: string | null;
  openLine: number;
  parent: OpenSubgraph | null;
  /** Ids the statements directly inside name, child subgraph ids included. */
  direct: string[];
  closed: Subgraph | null;
}

/** A vertex mermaid's db creates or updates, in source order. */
type VertexEvent = { id: string; ref: NodeRef | null };

export function parseFlowchart(body: string): FlowModel {
  const model: FlowModel = {
    kind: "flowchart",
    keyword: "",
    direction: null,
    headerLine: -1,
    nodes: new Map(),
    nodeOrder: [],
    edges: [],
    chains: [],
    directives: [],
    subgraphs: [],
    scan: {
      lines: [],
      header: null,
      keyword: null,
      understood: false,
      reasons: [],
    },
    body,
    understood: false,
    reasons: [],
  };
  if (body.length > MAX_TEXT_SIZE) {
    model.reasons.push(
      `The diagram is too long: mermaid draws at most ${String(MAX_TEXT_SIZE)} characters.`,
    );
    return model;
  }
  try {
    const scan = scanFence(body);
    model.scan = scan;
    model.keyword = scan.keyword ?? "";
    model.headerLine = scan.header?.index ?? -1;
    model.reasons.push(...scan.reasons);
    new Parser(body, model).run();
  } catch (error) {
    model.reasons.push(
      `The parser stopped: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  model.understood = model.reasons.length === 0;
  return model;
}

/** The edge `key` names, or null when the model has no such edge. */
export function edgeByKey(model: FlowModel, key: EdgeKey): FlowEdge | null {
  return (
    model.edges.find(
      (e) =>
        e.start === key.start &&
        e.end === key.end &&
        e.occurrence === key.occurrence,
    ) ?? null
  );
}

class Parser {
  private readonly events: VertexEvent[] = [];
  private readonly stack: OpenSubgraph[] = [];
  private readonly all: OpenSubgraph[] = [];
  private readonly closed: Subgraph[] = [];
  /** Everything the closed subgraphs hold: mermaid's `makeUniq` skips these. */
  private readonly held = new Set<string>();
  private readonly usedEdgeIds = new Set<string>();
  private readonly pairCount = new Map<string, number>();
  private statement = -1;

  private readonly body: string;
  private readonly model: FlowModel;

  constructor(body: string, model: FlowModel) {
    this.body = body;
    this.model = model;
  }

  run(): void {
    const { model } = this;
    const keyword = model.scan.keyword;
    if (keyword === "flowchart-elk") {
      model.reasons.push("flowchart-elk diagrams are not edited yet.");
      return;
    }
    if (keyword === null || !KEYWORDS.has(keyword)) {
      model.reasons.push("The diagram is not a flowchart.");
      return;
    }
    for (const line of model.scan.lines) {
      if (
        line.kind === "header" ||
        line.kind === "statement" ||
        line.kind === "acc"
      ) {
        if (line.statements.length > 0 || isDirectionLine(this.body, line)) {
          this.line(line);
        }
      }
    }
    for (const open of this.stack) {
      model.reasons.push(
        `Line ${open.openLine + 1}: the subgraph is never closed with end.`,
      );
    }
    this.finish();
  }

  private line(line: ScannedLine): void {
    const where = `Line ${line.index + 1}`;
    if (hasStrayComment(this.body, line)) {
      this.statement += line.statements.length;
      this.model.reasons.push(`${where}: a %% that is not a comment line.`);
      return;
    }
    if (isDirectionLine(this.body, line)) {
      this.statement += line.statements.length;
      this.direction(line);
      return;
    }
    for (const [k, span] of line.statements.entries()) {
      this.statement++;
      try {
        if (line.kind === "header" && k === 0) {
          this.header(span);
        } else {
          this.statementAt(span, line);
        }
      } catch (error) {
        if (!(error instanceof Refusal)) {
          throw error;
        }
        this.model.reasons.push(`${where}: ${error.message}`);
      }
    }
  }

  private text(span: Span): string {
    return this.body.slice(span.from, span.to);
  }

  private header(span: Span): void {
    const text = this.text(span);
    const match =
      /^(flowchart|graph)(?:([ \t]+)(TB|TD|BT|RL|LR|<|>|\^|v))?$/.exec(text);
    if (!match) {
      refuse("the keyword line holds more than a direction.");
    }
    const value = match[3];
    if (value !== undefined) {
      const from =
        span.from + (match[1] ?? "").length + (match[2] ?? "").length;
      this.model.direction = {
        value,
        span: { from, to: from + value.length },
      };
    }
  }

  /** R7: only a plain `direction XX` inside a subgraph is understood. */
  private direction(line: ScannedLine): void {
    const where = `Line ${line.index + 1}`;
    const span = line.statements[0];
    const match =
      span !== undefined && line.statements.length === 1
        ? /^direction(\s+)(TB|BT|RL|LR|TD)$/.exec(this.text(span))
        : null;
    if (!match || span === undefined || line.kind !== "statement") {
      this.model.reasons.push(
        `${where}: mermaid reads this whole line as a direction.`,
      );
      return;
    }
    if (this.stack.length === 0) {
      this.model.reasons.push(`${where}: a direction line outside a subgraph.`);
      return;
    }
    const valueFrom = span.from + "direction".length + (match[1] ?? "").length;
    this.model.directives.push({
      kind: "direction",
      line: line.index,
      span,
      targets: [],
      props: [
        {
          key: "direction",
          key_: { from: span.from, to: span.from + "direction".length },
          value: { from: valueFrom, to: span.to },
          entry: span,
        },
      ],
      indices: [],
      className: null,
    });
  }

  private statementAt(span: Span, line: ScannedLine): void {
    const text = this.text(span);
    if (/^subgraph\b/.test(text)) {
      this.openSubgraph(span, line);
    } else if (text === "end") {
      this.closeSubgraph(line);
    } else if (/^style\s/.test(text)) {
      this.style(span, line);
    } else if (/^classDef\s/.test(text)) {
      this.classDef(span, line);
    } else if (/^class\s/.test(text)) {
      this.classLine(span, line);
    } else if (/^click\s/.test(text)) {
      this.click(span, line);
    } else if (/^linkStyle\s/.test(text)) {
      this.linkStyle(span, line);
    } else {
      this.chain(span, line);
    }
  }

  // ---- subgraphs ----

  private openSubgraph(span: Span, line: ScannedLine): void {
    const rest = this.text(span).slice("subgraph".length);
    let named: string | null;
    let title: string | null = null;
    if (rest.trim() === "") {
      // mermaid 12.2.0 throws on a subgraph line without a title.
      refuse("a subgraph line without an id or title.");
    } else {
      const words = rest.trim();
      if (!/^\s/.test(rest)) {
        refuse("a subgraph line it cannot read.");
      }
      const titled = /^([^\s[]+)\s*\[([^[\]]*)\]$/.exec(words);
      if (titled) {
        named = this.subgraphId(defined(titled[1]));
        title = defined(titled[2]);
      } else if (/^"[^"]*"$/.test(words)) {
        if (!/\s/.test(words.slice(1, -1))) {
          refuse("a quoted subgraph title without a space.");
        }
        named = null;
        title = words;
      } else if (/\s/.test(words)) {
        if (
          !/^[A-Za-z0-9_ .,:'!?-]+$/.test(words) ||
          /--|==|-\.|\.-/.test(words) ||
          words.split(/\s+/).some((w) => RESERVED.test(w))
        ) {
          refuse("a subgraph title it cannot read.");
        }
        named = null;
        title = words;
      } else {
        named = this.subgraphId(words);
      }
    }
    const open: OpenSubgraph = {
      named,
      title,
      openLine: line.index,
      parent: this.stack[this.stack.length - 1] ?? null,
      direct: [],
      closed: null,
    };
    this.stack.push(open);
    this.all.push(open);
  }

  private subgraphId(id: string): string {
    if (!PLAIN_ID.test(id) || RESERVED.test(id)) {
      refuse(`the subgraph id ${id} is not one it knows.`);
    }
    return id;
  }

  private closeSubgraph(line: ScannedLine): void {
    const open = this.stack.pop();
    if (open === undefined) {
      refuse("an end without a subgraph.");
    }
    // mermaid's addSubGraph runs at `end`, so the counter is in close order.
    const id = open.named ?? `subGraph${this.closed.length}`;
    if (this.closed.some((s) => s.id === id)) {
      refuse(`the subgraph id ${id} is used twice.`);
    }
    const members = [
      ...new Set(open.direct.filter((m) => m !== id && !this.held.has(m))),
    ];
    for (const member of members) {
      this.held.add(member);
    }
    const subgraph: Subgraph = {
      id,
      title: open.title,
      openLine: open.openLine,
      endLine: line.index,
      parent: null,
      members,
    };
    open.closed = subgraph;
    this.closed.push(subgraph);
    open.parent?.direct.push(id);
  }

  // ---- directive lines ----

  private directive(
    kind: DirectiveLine["kind"],
    span: Span,
    line: ScannedLine,
  ): DirectiveLine {
    const directive: DirectiveLine = {
      kind,
      line: line.index,
      span,
      targets: [],
      props: [],
      indices: [],
      className: null,
    };
    this.model.directives.push(directive);
    return directive;
  }

  /** `<keyword> <word> <rest>` split into body spans. */
  private words(
    span: Span,
    keyword: string,
  ): { word: Span; rest: Span | null } {
    const text = this.text(span);
    const match = new RegExp(`^${keyword}(\\s+)(\\S+)(?:(\\s+)(.+))?$`).exec(
      text,
    );
    if (!match) {
      refuse(`a ${keyword} line it cannot read.`);
    }
    const wordFrom = span.from + keyword.length + (match[1] ?? "").length;
    const word = { from: wordFrom, to: wordFrom + (match[2] ?? "").length };
    const rest =
      match[4] === undefined
        ? null
        : { from: word.to + (match[3] ?? "").length, to: span.to };
    return { word, rest };
  }

  /** A class name mermaid reads whole: its NUM rule would end one after leading digits. */
  private className(name: string): void {
    if (!PLAIN_ID.test(name) || /^\d/.test(name)) {
      refuse(`the class name ${name} is not one it knows.`);
    }
  }

  /** `a,b,c` as ids with their spans. */
  private list(span: Span, what: string): { id: string; span: Span }[] {
    const out: { id: string; span: Span }[] = [];
    let from = span.from;
    for (const part of this.text(span).split(",")) {
      if (!PLAIN_ID.test(part) || RESERVED.test(part)) {
        refuse(`the ${what} ${part} is not one it knows.`);
      }
      out.push({ id: part, span: { from, to: from + part.length } });
      from += part.length + 1;
    }
    return out;
  }

  /** CSS `key:value,key:value` entries. */
  private props(span: Span): DirectiveLine["props"] {
    const text = this.text(span);
    // A `;` here is one `encodeEntities` removes, which joins what follows
    // into this value: a whole second statement can sit in a value span.
    if (/[\\";]/.test(text)) {
      refuse("a style it cannot read.");
    }
    const out: DirectiveLine["props"] = [];
    let at = 0;
    for (const piece of text.split(",")) {
      const lead = piece.length - piece.trimStart().length;
      const entryText = piece.trim();
      const match = /^([A-Za-z-]+)(\s*:\s*)(\S(?:.*\S)?)$/.exec(entryText);
      if (!match) {
        refuse(`the style entry "${entryText}" is not key:value.`);
      }
      const from = span.from + at + lead;
      const key = match[1] ?? "";
      const valueFrom = from + key.length + (match[2] ?? "").length;
      out.push({
        key,
        key_: { from, to: from + key.length },
        value: { from: valueFrom, to: from + entryText.length },
        entry: { from, to: from + entryText.length },
      });
      at += piece.length + 1;
    }
    return out;
  }

  private style(span: Span, line: ScannedLine): void {
    const { word, rest } = this.words(span, "style");
    if (rest === null) {
      refuse("a style line without styles.");
    }
    const directive = this.directive("style", span, line);
    const id = this.text(word);
    if (!PLAIN_ID.test(id) || RESERVED.test(id)) {
      refuse(`the id ${id} is not one it knows.`);
    }
    directive.targets.push({ id, span: word });
    directive.props = this.props(rest);
    // `style` is the one directive that creates a vertex (addVertex).
    this.events.push({ id, ref: null });
  }

  private classDef(span: Span, line: ScannedLine): void {
    const { word, rest } = this.words(span, "classDef");
    if (rest === null) {
      refuse("a classDef line without styles.");
    }
    const directive = this.directive("classDef", span, line);
    directive.targets = this.list(word, "class name");
    for (const target of directive.targets) {
      this.className(target.id);
    }
    directive.props = this.props(rest);
  }

  private classLine(span: Span, line: ScannedLine): void {
    const { word, rest } = this.words(span, "class");
    if (rest === null || /\s/.test(this.text(rest))) {
      refuse("a class line it cannot read.");
    }
    const name = this.text(rest);
    this.className(name);
    const directive = this.directive("class", span, line);
    directive.targets = this.list(word, "id");
    directive.className = { name, span: rest };
  }

  private click(span: Span, line: ScannedLine): void {
    const { word, rest } = this.words(span, "click");
    if (rest === null) {
      refuse("a click line without a link or callback.");
    }
    const directive = this.directive("click", span, line);
    directive.targets = this.list(word, "id");
  }

  private linkStyle(span: Span, line: ScannedLine): void {
    const text = this.text(span);
    const match =
      /^linkStyle(\s+)(default|\d+(?:,\d+)*)(\s+)(?:(interpolate\s+\w+)(?:\s+|$))?(.*)$/.exec(
        text,
      );
    if (!match) {
      refuse("a linkStyle line it cannot read.");
    }
    const directive = this.directive("linkStyle", span, line);
    const listFrom = span.from + "linkStyle".length + (match[1] ?? "").length;
    const list = match[2] ?? "";
    if (list !== "default") {
      let from = listFrom;
      for (const digits of list.split(",")) {
        const n = Number(digits);
        if (n >= this.model.edges.length) {
          refuse(`linkStyle ${n} names an edge that does not exist yet.`);
        }
        directive.indices.push({ n, span: { from, to: from + digits.length } });
        from += digits.length + 1;
      }
    }
    const styles = match[5] ?? "";
    if (styles !== "") {
      directive.props = this.props({
        from: span.to - styles.length,
        to: span.to,
      });
    } else if (match[4] === undefined) {
      refuse("a linkStyle line without styles.");
    }
  }

  // ---- chains ----

  private chain(span: Span, line: ScannedLine): void {
    const reader = new ChainReader(this.body, span, this.statement, line.index);
    const { groups, arrows } = reader.read();
    const chainIndex = this.model.chains.length;
    const links: Link[] = arrows.map((arrow, k) => {
      const left = defined(groups[k]);
      const right = defined(groups[k + 1]);
      return {
        gap: { from: left.span.to, to: right.span.from },
        arrow,
        edges: this.edgesOf(left, right, arrow, chainIndex, k),
      };
    });
    const chain: Chain = {
      statement: this.statement,
      line: line.index,
      span,
      groups,
      links,
    };
    this.model.chains.push(chain);
    const open = this.stack[this.stack.length - 1];
    for (const group of groups) {
      const seen = new Set<string>();
      for (const ref of group.refs) {
        if (seen.has(ref.id)) {
          refuse(`${ref.id} is named twice in one & group.`);
        }
        seen.add(ref.id);
        this.events.push({ id: ref.id, ref });
        open?.direct.push(ref.id);
      }
    }
  }

  /** mermaid's addLink: start outer, end inner; an explicit id on (last start, first end). */
  private edgesOf(
    left: Group,
    right: Group,
    arrow: Arrow,
    chain: number,
    link: number,
  ): number[] {
    const explicit = arrow.idSpan ? this.text(arrow.idSpan) : null;
    const out: number[] = [];
    const lastStart = left.refs[left.refs.length - 1]?.id;
    const firstEnd = right.refs[0]?.id;
    for (const s of left.refs) {
      for (const e of right.refs) {
        if (this.model.edges.length >= MAX_EDGES) {
          refuse(`more than ${String(MAX_EDGES)} arrows.`);
        }
        const pair = `${s.id}\u0000${e.id}`;
        const k = this.pairCount.get(pair) ?? 0;
        const wanted =
          explicit !== null && s.id === lastStart && e.id === firstEnd
            ? explicit
            : null;
        const mermaidId =
          wanted !== null && !this.usedEdgeIds.has(wanted)
            ? wanted
            : `L_${s.id}_${e.id}_${k === 0 ? 0 : k + 1}`;
        this.usedEdgeIds.add(mermaidId);
        this.pairCount.set(pair, k + 1);
        const index = this.model.edges.length;
        this.model.edges.push({
          index,
          start: s.id,
          end: e.id,
          mermaidId,
          occurrence: k,
          chain,
          link,
        });
        out.push(index);
      }
    }
    return out;
  }

  // ---- after the pass ----

  private finish(): void {
    const { model } = this;
    for (const open of this.all) {
      const closed = open.closed;
      if (closed) {
        closed.parent = open.parent?.closed?.id ?? null;
      }
    }
    model.subgraphs = this.closed;
    const clusters = new Set(this.closed.map((s) => s.id));
    const edgeIds = new Set<string>();
    for (const { mermaidId } of model.edges) {
      // mermaid's render throws on two edges with one id, and an edge that
      // shares a subgraph's id shares its DOM id.
      if (edgeIds.has(mermaidId)) {
        model.reasons.push(`Two arrows have the id ${mermaidId}.`);
      }
      if (clusters.has(mermaidId)) {
        model.reasons.push(
          `The id ${mermaidId} names an arrow and a subgraph.`,
        );
      }
      edgeIds.add(mermaidId);
    }
    for (const { id, ref } of this.events) {
      if (edgeIds.has(id)) {
        model.reasons.push(`The id ${id} names both a box and an arrow.`);
        continue;
      }
      if (clusters.has(id)) {
        // R4: an edge end or style target that is a subgraph id is drawn
        // as the cluster, never as a box.
        if (ref && (ref.shape || ref.classSpan)) {
          model.reasons.push(
            `Line ${ref.line + 1}: the subgraph id ${id} is given a shape or class.`,
          );
        }
        continue;
      }
      let node = model.nodes.get(id);
      if (node === undefined) {
        node = { id, refs: [], defining: null, subgraph: null };
        model.nodes.set(id, node);
        model.nodeOrder.push(id);
      }
      if (ref) {
        node.refs.push(ref);
        if (ref.shape) {
          node.defining = ref;
        }
      }
    }
    for (const subgraph of this.closed) {
      for (const member of subgraph.members) {
        const node: FlowNode | undefined = model.nodes.get(member);
        if (node) {
          node.subgraph = subgraph.id;
        }
      }
    }
  }
}

/** Reads one chain statement: groups of refs joined by links. */
class ChainReader {
  private readonly text: string;
  private readonly span: Span;
  private readonly statement: number;
  private readonly line: number;
  private i = 0;
  private linkIdTable: Int32Array | null = null;

  constructor(body: string, span: Span, statement: number, line: number) {
    this.text = body.slice(span.from, span.to);
    this.span = span;
    this.statement = statement;
    this.line = line;
  }

  private at(i: number): number {
    return this.span.from + i;
  }

  private sticky(re: RegExp, i: number): RegExpExecArray | null {
    re.lastIndex = i;
    return re.exec(this.text);
  }

  /**
   * Where mermaid's LINK_ID (rule 61, `[^\s"]+@(?=[^{"])`) ends when it starts
   * at `i`, or -1. The regex would rescan the rest of the run at every ref, so
   * the last usable `@` of each run is looked up once per statement.
   */
  private linkIdEnd(i: number): number {
    if (this.linkIdTable === null) {
      const text = this.text;
      const table = new Int32Array(text.length).fill(-1);
      if (text.includes("@")) {
        let last = -1;
        for (let k = text.length - 1; k >= 0; k--) {
          const c = text.charAt(k);
          if (c === '"' || /\s/.test(c)) {
            last = -1;
            continue;
          }
          const next = text[k + 1];
          if (
            last === -1 &&
            c === "@" &&
            next !== undefined &&
            next !== "{" &&
            next !== '"'
          ) {
            last = k;
          }
          table[k] = last;
        }
      }
      this.linkIdTable = table;
    }
    const at = this.linkIdTable[i] ?? -1;
    return at > i ? at + 1 : -1;
  }

  read(): { groups: Group[]; arrows: Arrow[] } {
    const groups: Group[] = [this.group()];
    const arrows: Arrow[] = [];
    while (this.i < this.text.length) {
      arrows.push(this.link());
      if (this.i >= this.text.length) {
        refuse("an arrow with nothing after it.");
      }
      groups.push(this.group());
    }
    return { groups, arrows };
  }

  private group(): Group {
    const refs = [this.ref()];
    for (;;) {
      let j = this.i;
      while (j < this.text.length && /[ \t]/.test(this.text.charAt(j))) {
        j++;
      }
      if (this.text[j] !== "&") {
        break;
      }
      j++;
      while (j < this.text.length && /[ \t]/.test(this.text.charAt(j))) {
        j++;
      }
      this.i = j;
      refs.push(this.ref());
    }
    const first = defined(refs[0]);
    const last = defined(refs[refs.length - 1]);
    return { refs, span: { from: first.span.from, to: last.span.to } };
  }

  private ref(): NodeRef {
    const start = this.i;
    const rest = this.text.slice(start);
    if (RESERVED.test(rest)) {
      refuse(
        `a reserved word where a box id should be: ${rest.split(/\s/)[0]}.`,
      );
    }
    if (
      this.linkIdEnd(start) !== -1 ||
      LINK_RULES.some((r) => this.sticky(r.re, start))
    ) {
      refuse("an arrow or arrow id where a box id should be.");
    }
    const token = this.sticky(ID_TOKEN, start);
    if (!token) {
      refuse(`no box id at "${rest.slice(0, 12)}".`);
    }
    const id = token[0];
    // NUM (rule 62) comes before NODE_STRING, so a keyword after leading
    // digits is lexed as the keyword.
    const digits = /^\d+/.exec(id)?.[0].length ?? 0;
    if (!PLAIN_ID.test(id) || (digits > 0 && RESERVED.test(id.slice(digits)))) {
      refuse(`the box id ${id} uses characters it does not know.`);
    }
    this.i = start + id.length;
    const idSpan = { from: this.at(start), to: this.at(this.i) };
    let shape: Shape | null = null;
    if (this.text.startsWith("@{", this.i)) {
      shape = this.atShape();
    } else if (
      "[({>".includes(this.text.charAt(this.i)) &&
      this.i < this.text.length
    ) {
      shape = this.delimited();
    }
    let classSpan: Span | null = null;
    if (this.text.startsWith(":::", this.i)) {
      if (shape?.form === "at") {
        refuse("a class after @{ } shape data.");
      }
      const name = this.sticky(ID_TOKEN, this.i + 3);
      // mermaid's NUM rule ends a token after its leading digits, so a name
      // that starts with one is not the name it looks like.
      if (!name || !PLAIN_ID.test(name[0]) || /^\d/.test(name[0])) {
        refuse("a ::: without a class name it knows.");
      }
      classSpan = {
        from: this.at(this.i + 3),
        to: this.at(this.i + 3 + name[0].length),
      };
      this.i += 3 + name[0].length;
    }
    return {
      id,
      idSpan,
      shape,
      classSpan,
      span: { from: idSpan.from, to: this.at(this.i) },
      statement: this.statement,
      line: this.line,
    };
  }

  private delimited(): Shape {
    const text = this.text;
    if (text.startsWith("(-", this.i)) {
      // R5: the ellipse parses but mermaid 12.2.0 cannot draw it.
      refuse("the ellipse shape (-x-) does not draw.");
    }
    if (text.startsWith("[|", this.i)) {
      refuse("a box with [| |] properties.");
    }
    const opener = defined(
      OPENERS.find((o) => text.startsWith(o.open, this.i)),
    );
    const openFrom = this.i;
    const labelFrom = openFrom + opener.open.length;
    let j = labelFrom;
    let quoted = false;
    let markdown = false;
    if (text.startsWith('"`', j)) {
      const end = text.indexOf('`"', j + 2);
      if (end === -1 || /["`]/.test(text.slice(j + 2, end))) {
        refuse("a markdown label that does not close on its line.");
      }
      quoted = true;
      markdown = true;
      j = end + 2;
    } else if (text[j] === '"') {
      const end = text.indexOf('"', j + 1);
      if (end === -1) {
        refuse("a quoted label that does not close on its line.");
      }
      if (end === j + 1) {
        refuse("an empty quoted label.");
      }
      quoted = true;
      j = end + 1;
    } else {
      while (j < text.length) {
        const c = text.charAt(j);
        if (opener.trap && (c === "/" || c === "\\") && text[j + 1] === "]") {
          break;
        }
        if (LABEL_STOP.test(c)) {
          break;
        }
        j++;
      }
      const label = text.slice(labelFrom, j);
      if (label.trim() === "") {
        refuse("an empty label.");
      }
      if (label.includes("~~")) {
        refuse("a label with ~~ in it.");
      }
    }
    const closer = opener.closers.find((c) => text.startsWith(c.close, j));
    if (!closer) {
      refuse("a label that does not close where it should.");
    }
    this.i = j + closer.close.length;
    return {
      form: "delimiters",
      name: closer.name,
      open: { from: this.at(openFrom), to: this.at(labelFrom) },
      close: { from: this.at(j), to: this.at(this.i) },
      label: { from: this.at(labelFrom), to: this.at(j) },
      quoted,
      markdown,
      atShape: null,
    };
  }

  /** `@{ shape: x, label: "y" }` on one line, with those two keys only. */
  private atShape(): Shape {
    const text = this.text;
    const openFrom = this.i;
    let j = openFrom + 2;
    let inQuote = false;
    for (; j < text.length; j++) {
      const c = text[j];
      if (inQuote) {
        inQuote = c !== '"';
      } else if (c === '"') {
        inQuote = true;
      } else if (c === "}") {
        break;
      } else if (c === "^") {
        refuse("shape data with a ^ in it.");
      }
    }
    if (j >= text.length) {
      refuse("@{ } shape data that does not close on its line.");
    }
    const closeFrom = j;
    let shapeSpan: Span | null = null;
    let shapeValue = "";
    let label: Span | null = null;
    let quoted = false;
    let at = openFrom + 2;
    for (const piece of splitOutsideQuotes(
      text.slice(openFrom + 2, closeFrom),
    )) {
      const colon = piece.indexOf(":");
      const key = piece.slice(0, Math.max(colon, 0)).trim();
      if (!/^\w+$/.test(key)) {
        refuse("shape data it cannot read.");
      }
      const after = piece.slice(colon + 1);
      const value = after.trim();
      const from = at + colon + 1 + (after.length - after.trimStart().length);
      const span = { from: this.at(from), to: this.at(from + value.length) };
      if (key === "shape") {
        if (
          shapeSpan !== null ||
          !/^[a-z][a-z0-9-]*$/.test(value) ||
          SHAPES_WITHOUT_BOX.has(value)
        ) {
          refuse(`the shape ${value} is not one it knows.`);
        }
        shapeSpan = span;
        shapeValue = value;
      } else if (key === "label") {
        if (label !== null) {
          refuse("shape data with two labels.");
        }
        if (/^"[^"\\`]+"$/.test(value)) {
          quoted = true;
        } else if (
          !/^[A-Za-z][A-Za-z0-9 _.-]*$/.test(value) ||
          // YAML's JSON schema reads every casing of these as a boolean or
          // null, and mermaid stores that as the label.
          /^(?:true|false|null)$/i.test(value)
        ) {
          refuse(`the label value ${value} is not one it knows.`);
        }
        label = span;
      } else {
        refuse(`the shape data key ${key} is not one it knows.`);
      }
      at += piece.length + 1;
    }
    if (shapeSpan === null || label === null) {
      refuse("@{ } shape data needs both a shape and a label.");
    }
    this.i = closeFrom + 1;
    return {
      form: "at",
      name: AT_SHAPE_NAMES[shapeValue] ?? null,
      open: { from: this.at(openFrom), to: this.at(openFrom + 2) },
      close: { from: this.at(closeFrom), to: this.at(this.i) },
      label,
      quoted,
      markdown: false,
      atShape: shapeSpan,
    };
  }

  /** One link at `i`, from the end of a ref to the start of the next. */
  private link(): Arrow {
    const text = this.text;
    let start = this.i;
    let idSpan: Span | null = null;
    // LINK_ID (rule 61) is tried before the link rules, but it cannot start
    // on whitespace, which the link rules carry with them.
    let rule = /\s/.test(text.charAt(start)) ? this.ruleAt(start) : null;
    if (rule === null) {
      while (start < text.length && /[ \t]/.test(text.charAt(start))) {
        start++;
      }
      const idEnd = this.linkIdEnd(start);
      if (idEnd !== -1) {
        const name = text.slice(start, idEnd - 1);
        if (!PLAIN_ID.test(name)) {
          refuse(`the arrow id ${name} is not one it knows.`);
        }
        idSpan = { from: this.at(start), to: this.at(start + name.length) };
        start = idEnd;
      }
      rule = this.ruleAt(start);
    }
    if (rule === null) {
      refuse(`no arrow at "${text.slice(start, start + 12).trim()}".`);
    }
    const token = defined(this.sticky(rule.re, start))[0];
    const tokenEnd = start + token.length;
    const trimmed = trimSpan(token, start);
    if (rule.text === undefined) {
      const info = destructLink(text.slice(trimmed.from, trimmed.to), null);
      this.i = tokenEnd;
      const arrow = this.arrow(info, {
        span: { from: this.at(trimmed.from), to: this.at(trimmed.to) },
        form: "plain",
        idSpan,
      });
      if (text[this.i] === "|") {
        this.pipe(arrow);
      }
      return arrow;
    }
    // START_LINK: the label runs to the first closing token.
    const textRule = rule.text;
    let j = tokenEnd;
    let quoted = false;
    if (text[j] === '"') {
      const end = text.indexOf('"', j + 1);
      if (end === -1 || end === j + 1) {
        refuse("an arrow label that does not close on its line.");
      }
      quoted = true;
      j = end + 1;
    }
    let closeMatch: RegExpExecArray | null = null;
    for (; j <= text.length; j++) {
      closeMatch = this.sticky(textRule.close, j);
      if (closeMatch) {
        break;
      }
      if (
        j === text.length ||
        quoted ||
        !textRule.char(text, j) ||
        /[[\](){}|"~]/.test(text.charAt(j))
      ) {
        refuse("an arrow label it cannot read.");
      }
      // The closing rule starts with `\s*`: having failed at the start of a
      // run of whitespace, it fails at every later place in that run.
      while (/\s/.test(text.charAt(j)) && /\s/.test(text.charAt(j + 1))) {
        j++;
      }
    }
    if (!closeMatch) {
      refuse("an arrow label that does not close.");
    }
    const labelText = text.slice(tokenEnd, j);
    if (labelText.trim() === "") {
      refuse("an empty arrow label.");
    }
    const closer = trimSpan(closeMatch[0], j);
    const opener = text.slice(trimmed.from, trimmed.to);
    const info = destructLink(text.slice(closer.from, closer.to), opener);
    this.i = j + closeMatch[0].length;
    const arrow = this.arrow(info, {
      span: { from: this.at(trimmed.from), to: this.at(closer.to) },
      form: "text",
      idSpan,
    });
    arrow.label = { from: this.at(tokenEnd), to: this.at(j) };
    arrow.labelQuoted = quoted;
    arrow.opener = { from: this.at(trimmed.from), to: this.at(trimmed.to) };
    arrow.closer = { from: this.at(closer.from), to: this.at(closer.to) };
    if (text[this.i] === "|") {
      refuse("a pipe label after a text label.");
    }
    return arrow;
  }

  private ruleAt(i: number): LinkRule | null {
    return LINK_RULES.find((r) => this.sticky(r.re, i) !== null) ?? null;
  }

  private arrow(
    info: LinkInfo,
    parts: { span: Span; form: Arrow["form"]; idSpan: Span | null },
  ): Arrow {
    const heads = HEADS[info.type];
    const line = LINES[info.stroke];
    if (heads === undefined || line === undefined) {
      refuse("an arrow whose two ends do not match.");
    }
    return {
      span: parts.span,
      line,
      start: heads[0],
      end: heads[1],
      length: info.length,
      form: parts.form,
      label: null,
      labelQuoted: false,
      opener: null,
      closer: null,
      idSpan: parts.idSpan,
    };
  }

  /** `|label|` right after a plain token (whose whitespace is already eaten). */
  private pipe(arrow: Arrow): void {
    const text = this.text;
    const from = this.i + 1;
    const end = text.indexOf("|", from);
    if (end === -1) {
      refuse("a | label that does not close.");
    }
    const inner = text.slice(from, end);
    if (/^"[^"]+"$/.test(inner)) {
      arrow.labelQuoted = true;
    } else if (
      inner.trim() === "" ||
      /[[\](){}"]/.test(inner) ||
      inner.includes("~~")
    ) {
      refuse("a | label it cannot read.");
    }
    arrow.form = "pipe";
    arrow.label = { from: this.at(from), to: this.at(end) };
    this.i = end + 1;
    while (this.i < text.length && /[ \t]/.test(text.charAt(this.i))) {
      this.i++;
    }
  }
}

interface LinkInfo {
  type: string;
  stroke: string;
  length: number;
}

const HEADS: Record<string, [Head, Head]> = {
  arrow_point: ["none", "arrow"],
  arrow_open: ["none", "none"],
  arrow_circle: ["none", "circle"],
  arrow_cross: ["none", "cross"],
  double_arrow_point: ["arrow", "arrow"],
  double_arrow_circle: ["circle", "circle"],
  double_arrow_cross: ["cross", "cross"],
};

const LINES: Record<string, Arrow["line"]> = {
  normal: "solid",
  dotted: "dotted",
  thick: "thick",
  invisible: "invisible",
};

/**
 * mermaid 12.2.0's `destructLink`, plus one refusal of ours: a plain token
 * with a head only at its start (`<---`, `x-->`), which mermaid reads with
 * a length that counts the head.
 */
function destructLink(end: string, start: string | null): LinkInfo {
  const info = destructEndLink(end);
  if (start === null) {
    if (/^[xo<]/.test(end) && !info.type.startsWith("double_")) {
      return { type: "INVALID", stroke: "INVALID", length: 0 };
    }
    return info;
  }
  if (/^[xo<]/.test(end)) {
    return { type: "INVALID", stroke: "INVALID", length: 0 };
  }
  const startInfo = destructStartLink(start);
  if (startInfo.stroke !== info.stroke) {
    return { type: "INVALID", stroke: "INVALID", length: 0 };
  }
  if (startInfo.type === "arrow_open") {
    return { ...info };
  }
  if (startInfo.type !== info.type) {
    return { type: "INVALID", stroke: "INVALID", length: 0 };
  }
  return {
    type: `double_${startInfo.type}`,
    stroke: info.stroke,
    length: info.length,
  };
}

function destructStartLink(str: string): { type: string; stroke: string } {
  let type = "arrow_open";
  let rest = str;
  const first = str[0];
  if (first === "<") {
    type = "arrow_point";
    rest = str.slice(1);
  } else if (first === "x") {
    type = "arrow_cross";
    rest = str.slice(1);
  } else if (first === "o") {
    type = "arrow_circle";
    rest = str.slice(1);
  }
  let stroke = "normal";
  if (rest.includes("=")) {
    stroke = "thick";
  }
  if (rest.includes(".")) {
    stroke = "dotted";
  }
  return { type, stroke };
}

function destructEndLink(str: string): LinkInfo {
  let line = str.slice(0, -1);
  let type = "arrow_open";
  const last = str.slice(-1);
  const mirror: Record<string, [string, string]> = {
    x: ["arrow_cross", "x"],
    ">": ["arrow_point", "<"],
    o: ["arrow_circle", "o"],
  };
  const head = mirror[last];
  if (head) {
    type = head[0];
    if (str.startsWith(head[1])) {
      type = `double_${type}`;
      line = line.slice(1);
    }
  }
  let stroke = "normal";
  let length = line.length - 1;
  if (line.startsWith("=")) {
    stroke = "thick";
  }
  if (line.startsWith("~")) {
    stroke = "invisible";
  }
  const dots = [...line].filter((c) => c === ".").length;
  if (dots) {
    stroke = "dotted";
    length = dots;
  }
  return { type, stroke, length };
}

/** `token` (found at line offset `at`) without its outer whitespace. */
function trimSpan(token: string, at: number): Span {
  const lead = token.length - token.trimStart().length;
  return { from: at + lead, to: at + lead + token.trim().length };
}

/** `text` split at commas outside double quotes. */
function splitOutsideQuotes(text: string): string[] {
  const out: string[] = [];
  let at = 0;
  let quote = false;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '"') {
      quote = !quote;
    } else if (text[i] === "," && !quote) {
      out.push(text.slice(at, i));
      at = i + 1;
    }
  }
  out.push(text.slice(at));
  return out;
}

function defined<T>(value: T | undefined | null): T {
  if (value === undefined || value === null) {
    throw new Error("internal: a value the parser relied on is missing");
  }
  return value;
}
