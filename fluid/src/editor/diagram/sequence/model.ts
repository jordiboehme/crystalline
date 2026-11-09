/**
 * What the sequence parser hands to the ops and the drawing: a model of the
 * source that keeps the span of every name, arrow and text, so an op changes
 * only the bytes it means. Spans are body offsets; read them with `body.slice`.
 *
 * A record is one entry of mermaid's `records.messages`, in order. Its index is
 * what the drawing carries as `data-id="i<index>"`.
 */

import type { Span } from "../edits";
import type { Scan } from "../scan";

export type SeqHead = "arrow" | "open" | "cross" | "async";

export interface SeqArrow {
  /** The arrow token alone, without the `+` or `-` after it. */
  span: Span;
  line: "solid" | "dotted";
  head: SeqHead;
  /** `<<->>` or `<<-->>`; `head` is `arrow` then. */
  both: boolean;
  /** The `+` or `-` after the token. */
  activation: Span | null;
}

export interface Declaration {
  line: number;
  /** The whole statement, `create` included. */
  span: Span;
  keyword: "participant" | "actor";
  nameSpan: Span;
  /** The label: after `as`, or the value of `alias` in a `@{ }`. */
  alias: Span | null;
  create: boolean;
  inBox: boolean;
  /** `@{ ... }`, delimiters included. An alias added after the name goes after this. */
  config: Span | null;
}

export interface Participant {
  name: string;
  declarations: Declaration[];
  /** The line of its first mention, declared or not. */
  firstLine: number;
  /** The decoded text mermaid draws: the last alias, or the name. */
  label: string;
  /** mermaid's type: `participant`, `actor`, `database`, ... */
  type: string;
}

export interface SeqMessage {
  line: number;
  /** From the sender's name to the end of the text. */
  span: Span;
  from: string;
  to: string;
  fromSpan: Span;
  toSpan: Span;
  arrow: SeqArrow;
  /** Without the colon, outer space and a `wrap:` prefix. Null when there is no text. */
  text: Span | null;
  /** The message's own record; a `+` or `-` adds the next one. */
  record: number;
  /** Among the messages with the same from, to and decoded text. */
  occurrence: number;
}

export type SeqStatementKind =
  | "declaration"
  | "message"
  | "note"
  | "activate"
  | "deactivate"
  | "autonumber"
  | "blockStart"
  | "blockMiddle"
  | "blockEnd"
  | "boxStart"
  | "boxEnd"
  | "destroy"
  | "link"
  | "title"
  | "acc";

export interface SeqStatement {
  line: number;
  kind: SeqStatementKind;
  actors: { name: string; span: Span }[];
  records: number[];
  /** How many blocks and boxes are open around it. A block's own lines count as outside it. */
  depth: number;
  /**
   * The statement without its `;` or line break. A line can hold several. An
   * `accDescr { }` block's span is its text alone, and its `line` that text's.
   */
  span: Span;
  /** A note's, block line's or box's text, a declaration's alias, a `link` line's value. */
  text: Span | null;
}

/** One entry of mermaid's record list, as the source says it must be. */
export interface SeqRecord {
  statement: number;
  from: string | undefined;
  to: string | undefined;
  /** mermaid's `LINETYPE`. */
  type: number;
  text: Span | null;
  /** On an `autonumber` record. */
  autonumber?: { start?: number; step?: number; visible: boolean };
  /** On a note: mermaid's `PLACEMENT`. */
  placement?: number;
}

export interface SeqBox {
  /** Members in the order mermaid adds them. */
  actors: string[];
}

export interface SeqModel {
  kind: "sequence";
  body: string;
  participants: Map<string, Participant>;
  /** Names in the order mermaid draws them, left to right. */
  order: string[];
  messages: SeqMessage[];
  statements: SeqStatement[];
  records: SeqRecord[];
  boxes: SeqBox[];
  /** A created participant to the index of the record that creates it. */
  created: Map<string, number>;
  /** A destroyed participant to the index of the record that destroys it. */
  destroyed: Map<string, number>;
  scan: Scan;
  understood: boolean;
  reasons: string[];
}

/** What a click or a menu acts on. Survives an edit that moves spans. */
export type SeqTarget =
  | { kind: "canvas" }
  | { kind: "participant"; name: string }
  | {
      kind: "message";
      from: string;
      to: string;
      text: string;
      occurrence: number;
    };

/** What an adding op made: the new participant's or message's target. */
export type SeqCreated = Exclude<SeqTarget, { kind: "canvas" }>;

/** mermaid's `LINETYPE` for the arrows the model holds. */
export const ARROW_TYPES: readonly {
  line: SeqArrow["line"];
  head: SeqHead;
  both: boolean;
  type: number;
}[] = [
  { line: "solid", head: "arrow", both: false, type: 0 },
  { line: "dotted", head: "arrow", both: false, type: 1 },
  { line: "solid", head: "cross", both: false, type: 3 },
  { line: "dotted", head: "cross", both: false, type: 4 },
  { line: "solid", head: "open", both: false, type: 5 },
  { line: "dotted", head: "open", both: false, type: 6 },
  { line: "solid", head: "async", both: false, type: 24 },
  { line: "dotted", head: "async", both: false, type: 25 },
  { line: "solid", head: "arrow", both: true, type: 33 },
  { line: "dotted", head: "arrow", both: true, type: 34 },
];
