/**
 * Sequence ops compiled to minimal text edits: an op changes only the bytes it
 * means, every other line stays as written, and when that cannot be promised it
 * refuses.
 *
 * Every compile checks itself before it answers (`checkedSeq`): the edits are
 * applied, the result is parsed again, and it must be understood and read exactly
 * as the op says: the order of the participants, their labels and types, each
 * record, who is created and destroyed, and each box. That is what refuses a
 * delete that would take the message a `create` waits for. A delete that would
 * leave more activations open than before refuses here too, from the `+` and `-`
 * markers and the `activate` lines. The apply layer then asks mermaid's db as well,
 * which is the only layer that sees the other way round, a `-` with no `+` left:
 * mermaid throws on it.
 *
 * mermaid builds its chain of neighbours (`prevActor`, `nextActor`) as participants
 * are first read, and a message's width reaches a gap only through it. A second
 * `addActor` with a label resets that actor's links and its menu, so a label is
 * never added below a first mention: an op writes declarations on top instead,
 * bare ones for the participants before it, which keep the order and the chain.
 *
 * Spans are read from `model.body`, the string they point into.
 */

import type { OpResult, RefusalCode, Span, TextEdit } from "../edits";
import { applyEdits } from "../edits";
import { lineDeletions, statementDeletions } from "../flowchart/linkStyle";
import {
  drawsOtherwise,
  encodeConfigAlias,
  encodeSeqText,
  tidySeqText,
} from "./labels";
import {
  ARROW_TYPES,
  type Declaration,
  type SeqCreated,
  type SeqHead,
  type SeqMessage,
  type SeqModel,
  type SeqStatement,
  type SeqTarget,
} from "./model";
import { decodeSeqText, parseSequence } from "./parse";

type MessageTarget = SeqTarget & { kind: "message" };

export type SeqOp =
  | { op: "renameParticipant"; name: string; text: string }
  | { op: "renameMessage"; target: MessageTarget; text: string }
  | { op: "addParticipant" }
  | { op: "addMessage"; from: string; to: string }
  | {
      op: "arrowStyle";
      target: MessageTarget;
      line?: "solid" | "dotted";
      head?: SeqHead | "both";
    }
  | { op: "deleteMessage"; target: MessageTarget }
  | { op: "deleteParticipant"; name: string }
  /** The block's number among `blocksOf`. */
  | { op: "removeBlock"; block: number };

const NEW_PARTICIPANT = "New participant";
const NEW_MESSAGE = "New message";
const FALLBACK_INDENT = "    ";

const refused = (code: RefusalCode): OpResult<never> => ({ refused: code });

/** One record as the check compares it. */
interface RecordRow {
  from: string | undefined;
  to: string | undefined;
  type: number;
  text: string;
  placement: number | undefined;
  autonumber: string;
}

/** What a body reads as, the parts `agreesSequence` proves against mermaid. */
export interface Reading {
  /** Per participant, in mermaid's order: its label and type. */
  people: Map<string, { label: string; type: string }>;
  records: RecordRow[];
  boxes: string[][];
  created: Map<string, number>;
  destroyed: Map<string, number>;
}

export function readingOf(model: SeqModel): Reading {
  return {
    people: new Map(
      model.order.map((name) => {
        const p = model.participants.get(name);
        return [name, { label: p?.label ?? "", type: p?.type ?? "" }];
      }),
    ),
    records: model.records.map((record) => ({
      from: record.from,
      to: record.to,
      type: record.type,
      text: record.text
        ? decodeSeqText(model.body.slice(record.text.from, record.text.to))
        : "",
      placement: record.placement,
      autonumber: JSON.stringify(record.autonumber ?? null),
    })),
    boxes: model.boxes.map((box) => [...box.actors]),
    created: new Map(model.created),
    destroyed: new Map(model.destroyed),
  };
}

/** A map as one comparable string, whatever order it was built in. */
function sortedEntries(map: ReadonlyMap<string, unknown>): unknown[] {
  return [...map].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
}

export function sameReading(a: Reading, b: Reading): boolean {
  const flat = (r: Reading): string =>
    JSON.stringify([
      [...r.people],
      r.records,
      r.boxes,
      sortedEntries(r.created),
      sortedEntries(r.destroyed),
    ]);
  return flat(a) === flat(b);
}

/**
 * `edits`, when applying them leaves a body that is understood and reads exactly
 * as `wanted`. Otherwise `invalid`. A `;` mermaid would drop, a keyword read in a
 * name, a line that lands in a block's text: all of them show up here.
 */
function checkedSeq<C extends SeqCreated = never>(
  model: SeqModel,
  edits: TextEdit[],
  wanted: Reading,
  created?: C,
): OpResult<C> {
  const after = parseSequence(applyEdits(model.body, edits));
  if (!after.understood || !sameReading(readingOf(after), wanted)) {
    return refused("invalid");
  }
  return created === undefined ? { edits } : { edits, created };
}

/** A message by what its target says of it. */
function messageAt(
  model: SeqModel,
  target: MessageTarget,
): SeqMessage | undefined {
  return model.messages.find((m) => {
    if (
      m.from !== target.from ||
      m.to !== target.to ||
      m.occurrence !== target.occurrence
    ) {
      return false;
    }
    return textOf(model, m) === target.text;
  });
}

function textOf(model: SeqModel, message: SeqMessage): string {
  return message.text
    ? decodeSeqText(model.body.slice(message.text.from, message.text.to))
    : "";
}

function indentOf(model: SeqModel, line: number): string {
  return model.scan.lines[line]?.indent ?? FALLBACK_INDENT;
}

/** The edit that adds `text` as a new line after line `index`, every old break on its own line (R51). */
function newLineAfter(
  model: SeqModel,
  index: number,
  text: string,
): TextEdit | null {
  const lines = model.scan.lines;
  const line = lines[index];
  if (line === undefined) {
    return null;
  }
  const next = lines[index + 1];
  return next
    ? { from: next.from, to: next.from, insert: `${text}\n` }
    : { from: line.to, to: line.to, insert: `\n${text}` };
}

/** The last line of the body that is not blank. */
function lastLine(model: SeqModel): number {
  return model.scan.lines.findLastIndex((line) => line.kind !== "blank");
}

/** The indent a new statement at the end of the body takes. */
function topLevelIndent(model: SeqModel): string {
  // The last one is at the top level: an open block is refused, and its `end` counts
  // as outside it. An `accDescr { }` block's statement sits on its text's line.
  const last = model.statements.findLast((s) => s.kind !== "acc");
  return last === undefined ? FALLBACK_INDENT : indentOf(model, last.line);
}

/**
 * Whether mermaid wraps the text that would start at `from`: a `wrap:` in front of
 * it, or a config naming `wrap`. Its wrapping measures the hidden entity forms and
 * can break a word inside one, which then draws as pieces and stray marker
 * characters, so an op does not write a form into such a text. `drawnLabels` can
 * rejoin `&...;` but not mermaid's markers. Seen in Chromium: `<b>x</b>` after a
 * `wrap:` on a message to itself draws `<b>x</b-` and `>`.
 */
function wrapsAt(model: SeqModel, line: number, from: number): boolean {
  const start = model.scan.lines[line]?.from ?? from;
  return (
    /(?:^|[\s:])wrap:\s*$/.test(model.body.slice(start, from)) ||
    model.scan.lines.some(
      (l) =>
        (l.kind === "frontmatter" || l.kind === "directive") &&
        model.body.slice(l.from, l.to).includes("wrap"),
    )
  );
}

const holdsForm = (encoded: string): boolean => /#\w+;/.test(encoded);

function renameMessage(
  model: SeqModel,
  target: MessageTarget,
  typed: string,
): OpResult<never> {
  const message = messageAt(model, target);
  if (message === undefined) {
    return refused("gone");
  }
  const text = tidySeqText(typed);
  if (textOf(model, message) === text) {
    return { edits: [] };
  }
  if (drawsOtherwise(text)) {
    return refused("invalid");
  }
  const wanted = readingOf(model);
  const row = wanted.records[message.record];
  if (row === undefined) {
    return refused("invalid");
  }
  row.text = text;
  const body = model.body;
  const encoded = encodeSeqText(text);
  if (
    holdsForm(encoded) &&
    wrapsAt(model, message.line, message.text?.from ?? message.span.to)
  ) {
    return refused("invalid");
  }
  if (message.text === null) {
    // The colon is already there (`A->>B` alone is refused), and `span.to` sits
    // right after it or after a `wrap:` prefix or the space that follows (R61).
    const at = message.span.to;
    const spaced = /\s/.test(body.charAt(at - 1));
    return checkedSeq(
      model,
      [{ from: at, to: at, insert: spaced ? encoded : ` ${encoded}` }],
      wanted,
    );
  }
  if (text === "") {
    let from = message.text.from;
    while (/\s/.test(body.charAt(from - 1))) {
      from--;
    }
    return checkedSeq(
      model,
      [{ from, to: message.text.to, insert: "" }],
      wanted,
    );
  }
  return checkedSeq(model, [{ ...message.text, insert: encoded }], wanted);
}

/** The edit that writes `text` over `declaration`'s alias. */
function aliasEdit(
  model: SeqModel,
  declaration: Declaration,
  alias: Span,
  text: string,
): TextEdit {
  const config = declaration.config;
  if (config === null || alias.from < config.from || alias.to > config.to) {
    return { ...alias, insert: encodeSeqText(text) };
  }
  const encoded = encodeConfigAlias(text);
  return {
    ...alias,
    insert:
      model.body.charAt(alias.from - 1) === '"' ? encoded : `"${encoded}"`,
  };
}

/** The statement where mermaid first adds a participant. `activate` and `destroy` add nobody. */
function firstAdd(model: SeqModel, name: string): SeqStatement | undefined {
  return model.statements.find(
    (s) =>
      (s.kind === "declaration" ||
        s.kind === "message" ||
        s.kind === "note" ||
        s.kind === "link") &&
      s.actors.some((actor) => actor.name === name),
  );
}

/** The outermost block or box around `statement`, or `statement` itself. */
function outermost(model: SeqModel, statement: SeqStatement): SeqStatement {
  if (statement.depth === 0) {
    return statement;
  }
  const index = model.statements.indexOf(statement);
  return (
    model.statements
      .slice(0, index)
      .findLast(
        (s) =>
          s.depth === 0 && (s.kind === "blockStart" || s.kind === "boxStart"),
      ) ?? statement
  );
}

/**
 * The line declarations for a mention in `statement` go above: the outermost block
 * around it, or a `destroy` that waits for this very message.
 */
function topLine(model: SeqModel, statement: SeqStatement): number {
  const holder = outermost(model, statement);
  const previous = model.statements[model.statements.indexOf(holder) - 1];
  return previous?.kind === "destroy" ? previous.line : holder.line;
}

/**
 * A bare declaration per name not read yet at line `line`, which keeps mermaid's
 * order and chain as they were. Null when one cannot take it: its own first
 * statement is a declaration with a label, which would then be a second `addActor`,
 * or a `create` of someone already there. A type that would be lost shows in the check.
 */
function bareDeclarations(
  model: SeqModel,
  names: readonly string[],
  line: number,
): string[] | null {
  const out: string[] = [];
  for (const name of names) {
    const participant = model.participants.get(name);
    if (participant === undefined || participant.firstLine < line) {
      continue;
    }
    const first = firstAdd(model, name);
    if (first?.kind === "declaration") {
      const declared = participant.declarations[0];
      if (
        declared === undefined ||
        declared.create ||
        declared.alias !== null
      ) {
        return null;
      }
    }
    out.push(`participant ${name}`);
  }
  return out;
}

/**
 * The edit that puts `texts` as lines above line `line`, at its indent. When that
 * line goes too and it is the body's last, its start lies inside the range that takes
 * it, so they go in front of the range and the state's break comes with them.
 */
function linesAbove(
  model: SeqModel,
  line: number,
  texts: readonly string[],
  removed: readonly Span[] = [],
): TextEdit | null {
  const start = model.scan.lines[line]?.from;
  if (start === undefined) {
    return null;
  }
  const indent = indentOf(model, line);
  const inside = removed.find((r) => r.from < start && start < r.to);
  return inside
    ? {
        from: inside.from,
        to: inside.from,
        insert: texts.map((text) => `\n${indent}${text}`).join(""),
      }
    : {
        from: start,
        to: start,
        insert: texts.map((text) => `${indent}${text}\n`).join(""),
      };
}

function renameParticipant(
  model: SeqModel,
  name: string,
  typed: string,
): OpResult<never> {
  const participant = model.participants.get(name);
  if (participant === undefined) {
    return refused("gone");
  }
  const text = tidySeqText(typed);
  if (text === "" || drawsOtherwise(text)) {
    return refused("invalid");
  }
  if (participant.label === text) {
    return { edits: [] };
  }
  const wanted = readingOf(model);
  const person = wanted.people.get(name);
  if (person === undefined) {
    return refused("invalid");
  }
  person.label = text;

  const named = participant.declarations.findLast((d) => d.alias !== null);
  const at = named?.alias?.from ?? participant.declarations[0]?.nameSpan.to;
  if (
    holdsForm(encodeSeqText(text)) &&
    wrapsAt(model, named?.line ?? participant.firstLine, at ?? 0)
  ) {
    return refused("invalid");
  }
  if (named?.alias) {
    return checkedSeq(
      model,
      [aliasEdit(model, named, named.alias, text)],
      wanted,
    );
  }
  // `participant Web App as L` is one participant named all of that: rule 12 takes
  // no spaces and rule 13 takes them all. No form gives such a name an alias (R61).
  if (/\s/.test(name)) {
    return refused("unsupported");
  }
  const encoded = encodeSeqText(text);
  const declared = participant.declarations[0];
  const first = firstAdd(model, name);
  if (first === undefined) {
    return refused("invalid");
  }
  if (declared && first.kind === "declaration") {
    const after = declared.config?.to ?? declared.nameSpan.to;
    return checkedSeq(
      model,
      [{ from: after, to: after, insert: ` as ${encoded}` }],
      wanted,
    );
  }
  // A label on a declaration below a first mention is a second `addActor`, which
  // resets the chain and takes a menu line above it out of the drawing (R69). So the
  // label goes on top. With a menu line standing before the declaration, it refuses.
  if (declared && menuBefore(model, name, declared)) {
    return refused("invalid");
  }
  const line = topLine(model, first);
  const earlier = bareDeclarations(
    model,
    model.order.slice(0, model.order.indexOf(name)),
    line,
  );
  const edit =
    earlier &&
    linesAbove(model, line, [...earlier, `participant ${name} as ${encoded}`]);
  return edit ? checkedSeq(model, [edit], wanted) : refused("invalid");
}

/** Whether a `link`, `links`, `properties` or `details` line of `name` stands before `declared`. */
function menuBefore(
  model: SeqModel,
  name: string,
  declared: Declaration,
): boolean {
  return model.statements.some(
    (s) =>
      s.kind === "link" &&
      s.span.from < declared.span.from &&
      s.actors.some((actor) => actor.name === name),
  );
}

function freeParticipantId(model: SeqModel): string {
  let k = 1;
  while (model.participants.has(`p${String(k)}`)) {
    k++;
  }
  return `p${String(k)}`;
}

/**
 * The line a new participant follows and that line's indent: the last declaration
 * (a box's `end` for one in a box) when every participant is declared before the
 * first message, else the end of the body. It lands rightmost either way.
 */
function newParticipantPlace(model: SeqModel): {
  line: number;
  indent: string;
} {
  const end = { line: lastLine(model), indent: topLevelIndent(model) };
  const firstMessage = model.messages[0]?.line ?? Number.POSITIVE_INFINITY;
  const everyDeclared = model.order.every((name) => {
    const first = model.participants.get(name)?.declarations[0];
    return first !== undefined && !first.create && first.line < firstMessage;
  });
  const statements = model.statements;
  // A declaration below the first message reads the actor again, and the new line
  // above it would change its neighbours.
  if (
    statements.some((s) => s.kind === "declaration" && s.line >= firstMessage)
  ) {
    return end;
  }
  const declarationOf = (s: SeqStatement): Declaration | undefined =>
    model.participants
      .get(s.actors[0]?.name ?? "")
      ?.declarations.find((d) => d.span.from === s.span.from);
  const index = statements.findLastIndex(
    (s) => s.kind === "declaration" && s.line < firstMessage,
  );
  const last = statements[index];
  if (!everyDeclared || last === undefined) {
    return end;
  }
  const anchor = declarationOf(last)?.inBox
    ? statements.slice(index).find((s) => s.kind === "boxEnd")
    : last;
  // Inside a block it would still draw, but the op's place is the top level.
  if (anchor === undefined || anchor.depth !== 0) {
    return end;
  }
  return { line: anchor.line, indent: indentOf(model, anchor.line) };
}

function addParticipant(model: SeqModel): OpResult<SeqCreated> {
  const name = freeParticipantId(model);
  const place = newParticipantPlace(model);
  const edit = newLineAfter(
    model,
    place.line,
    `${place.indent}participant ${name} as ${NEW_PARTICIPANT}`,
  );
  if (edit === null) {
    return refused("invalid");
  }
  const wanted = readingOf(model);
  wanted.people.set(name, { label: NEW_PARTICIPANT, type: "participant" });
  return checkedSeq(model, [edit], wanted, { kind: "participant", name });
}

function addMessage(
  model: SeqModel,
  from: string,
  to: string,
): OpResult<SeqCreated> {
  if (!model.participants.has(from) || !model.participants.has(to)) {
    return refused("gone");
  }
  // It would draw without a throw, with an arrow to a life line that has stopped.
  if (model.destroyed.has(from) || model.destroyed.has(to)) {
    return refused("invalid");
  }
  const edit = newLineAfter(
    model,
    lastLine(model),
    `${topLevelIndent(model)}${from}->>${to}: ${NEW_MESSAGE}`,
  );
  if (edit === null) {
    return refused("invalid");
  }
  const wanted = readingOf(model);
  wanted.records.push({
    from,
    to,
    type: 0,
    text: NEW_MESSAGE,
    placement: undefined,
    autonumber: "null",
  });
  const occurrence = model.messages.filter(
    (m) => m.from === from && m.to === to && textOf(model, m) === NEW_MESSAGE,
  ).length;
  return checkedSeq(model, [edit], wanted, {
    kind: "message",
    from,
    to,
    text: NEW_MESSAGE,
    occurrence,
  });
}

const ARROW_TOKENS: Readonly<Record<SeqHead | "both", string>> = {
  arrow: "->>",
  open: "->",
  cross: "-x",
  async: "-)",
  both: "<<->>",
};

function arrowToken(line: "solid" | "dotted", head: SeqHead | "both"): string {
  const token = ARROW_TOKENS[head];
  return line === "solid" ? token : token.replace("-", "--");
}

function arrowStyle(
  model: SeqModel,
  target: MessageTarget,
  wantedLine: "solid" | "dotted" | undefined,
  wantedHead: SeqHead | "both" | undefined,
): OpResult<never> {
  const message = messageAt(model, target);
  if (message === undefined) {
    return refused("gone");
  }
  const arrow = message.arrow;
  const line = wantedLine ?? arrow.line;
  const head = wantedHead ?? (arrow.both ? "both" : arrow.head);
  const both = head === "both";
  const shape = both ? "arrow" : head;
  if (arrow.line === line && arrow.head === shape && arrow.both === both) {
    return { edits: [] };
  }
  const type = ARROW_TYPES.find(
    (a) => a.line === line && a.head === shape && a.both === both,
  )?.type;
  const wanted = readingOf(model);
  const row = wanted.records[message.record];
  if (type === undefined || row === undefined) {
    return refused("invalid");
  }
  row.type = type;
  return checkedSeq(
    model,
    [{ ...arrow.span, insert: arrowToken(line, head) }],
    wanted,
  );
}

/**
 * Whether taking the statements `gone` away leaves a block section with no message
 * or note that had one (R56). A nested block's content counts for the sections
 * around it. A section written empty by hand stays as it is and stops nothing.
 */
function emptiesSection(
  model: SeqModel,
  gone: ReadonlySet<SeqStatement>,
): boolean {
  const open: { before: number; after: number }[] = [];
  const emptied = (section?: { before: number; after: number }): boolean =>
    section !== undefined && section.before > 0 && section.after === 0;
  for (const statement of model.statements) {
    switch (statement.kind) {
      case "blockStart":
        open.push({ before: 0, after: 0 });
        break;
      case "blockMiddle":
        if (emptied(open.pop())) {
          return true;
        }
        open.push({ before: 0, after: 0 });
        break;
      case "blockEnd":
        if (emptied(open.pop())) {
          return true;
        }
        break;
      case "message":
      case "note":
        for (const section of open) {
          section.before++;
          section.after += gone.has(statement) ? 0 : 1;
        }
        break;
      default:
        break;
    }
  }
  return false;
}

/**
 * The ranges that take `gone` out of the body. A line left with nothing but space
 * and `;` goes with its line break, through one `lineDeletions` call so neighbours
 * cannot overlap (R51). On a line that keeps anything else only the statements go,
 * with the `;` beside each run.
 */
function statementRemovals(
  model: SeqModel,
  gone: ReadonlySet<SeqStatement>,
): Span[] {
  const body = model.body;
  const byLine = new Map<number, SeqStatement[]>();
  for (const statement of model.statements) {
    const list = byLine.get(statement.line);
    if (list === undefined) {
      byLine.set(statement.line, [statement]);
    } else {
      list.push(statement);
    }
  }
  const wholeLines: number[] = [];
  const parts: Span[] = [];
  for (const [index, statements] of byLine) {
    if (!statements.some((s) => gone.has(s))) {
      continue;
    }
    const line = model.scan.lines[index];
    if (line === undefined) {
      continue;
    }
    let rest = "";
    let at = line.from;
    for (const statement of statements) {
      if (gone.has(statement)) {
        rest += body.slice(at, statement.span.from);
        at = statement.span.to;
      }
    }
    rest += body.slice(at, line.to);
    if (/^[\s;]*$/.test(rest)) {
      wholeLines.push(index);
      continue;
    }
    parts.push(
      ...statementDeletions(
        statements.map((statement) => statement.span),
        new Set(
          statements.filter((st) => gone.has(st)).map((st) => st.span.from),
        ),
      ),
    );
  }
  return [...lineDeletions(model, wholeLines), ...parts];
}

/** `wanted` without the records of `gone`, with each index that points past them moved. */
function withoutRecords(wanted: Reading, gone: ReadonlySet<number>): void {
  const moved = (index: number): number =>
    index - [...gone].filter((g) => g < index).length;
  wanted.records = wanted.records.filter((_, i) => !gone.has(i));
  wanted.created = new Map(
    [...wanted.created].map(([name, i]) => [name, moved(i)]),
  );
  wanted.destroyed = new Map(
    [...wanted.destroyed].map(([name, i]) => [name, moved(i)]),
  );
}

/** Per participant, the activations left open by the statements that are not in `gone`. */
function openActivations(
  model: SeqModel,
  gone: ReadonlySet<SeqStatement>,
): Map<string, number> {
  const open = new Map<string, number>();
  const add = (name: string | undefined, by: number): void => {
    if (name !== undefined) {
      open.set(name, (open.get(name) ?? 0) + by);
    }
  };
  for (const statement of model.statements) {
    if (gone.has(statement)) {
      continue;
    }
    if (statement.kind === "activate") {
      add(statement.actors[0]?.name, 1);
    } else if (statement.kind === "deactivate") {
      add(statement.actors[0]?.name, -1);
    }
  }
  for (const message of model.messages) {
    const statement =
      model.statements[model.records[message.record]?.statement ?? -1];
    const sign = message.arrow.activation;
    if (statement && !gone.has(statement) && sign) {
      if (model.body.slice(sign.from, sign.to) === "+") {
        add(message.to, 1);
      } else {
        add(message.from, -1);
      }
    }
  }
  return open;
}

/**
 * Whether taking `gone` away leaves anybody with more activations open: the source
 * would keep its `+`, and its bar would go from the drawing without a word.
 */
function raisesActivations(
  model: SeqModel,
  gone: ReadonlySet<SeqStatement>,
): boolean {
  const before = openActivations(model, new Set());
  for (const [name, count] of openActivations(model, gone)) {
    if (count > (before.get(name) ?? 0)) {
      return true;
    }
  }
  return false;
}

/**
 * `ranges` out of the body. When that would move or lose a participant that was first
 * read in `gone`, bare declarations on top keep it where it was (R69).
 */
function checkedRemoval(
  model: SeqModel,
  gone: ReadonlySet<SeqStatement>,
  ranges: readonly Span[],
  wanted: Reading,
  leaving?: string,
): OpResult<never> {
  const edits = ranges.map((span) => ({ ...span, insert: "" }));
  const plain = checkedSeq(model, edits, wanted);
  if (!("refused" in plain)) {
    return plain;
  }
  const names = model.order.filter((name) => name !== leaving);
  const first = (name: string) => firstAdd(model, name) as SeqStatement;
  const needed = names.filter((name) => gone.has(first(name)));
  const last = needed.at(-1);
  if (last === undefined) {
    return plain;
  }
  const line = Math.min(...needed.map((name) => topLine(model, first(name))));
  const lines = bareDeclarations(
    model,
    names.slice(0, names.indexOf(last) + 1),
    line,
  );
  const edit = lines && linesAbove(model, line, lines, ranges);
  return edit ? checkedSeq(model, [edit, ...edits], wanted) : plain;
}

function deleteMessage(
  model: SeqModel,
  target: MessageTarget,
): OpResult<never> {
  const message = messageAt(model, target);
  const statement = message
    ? model.statements[model.records[message.record]?.statement ?? -1]
    : undefined;
  if (statement === undefined) {
    return refused("gone");
  }
  const gone = new Set([statement]);
  if (emptiesSection(model, gone)) {
    return refused("emptiesBlock");
  }
  if (raisesActivations(model, gone)) {
    return refused("leavesActive");
  }
  const wanted = readingOf(model);
  withoutRecords(wanted, new Set(statement.records));
  return checkedRemoval(model, gone, statementRemovals(model, gone), wanted);
}

/**
 * Each block's own statements (its start, each `else`, `and` or `option`, its `end`),
 * in source order of the starts. A `box` is no block. The records on these are what
 * the drawing names: `control-structure` carries the record of a block's end.
 */
export function blocksOf(model: SeqModel): SeqStatement[][] {
  const blocks: SeqStatement[][] = [];
  const open: SeqStatement[][] = [];
  for (const statement of model.statements) {
    if (statement.kind === "blockStart") {
      const block = [statement];
      blocks.push(block);
      open.push(block);
    } else if (statement.kind === "blockMiddle") {
      open.at(-1)?.push(statement);
    } else if (statement.kind === "blockEnd") {
      open.pop()?.push(statement);
    }
  }
  return blocks;
}

/** R66: only the block's own keyword lines go. What it holds stays, with its indent. */
function removeBlock(model: SeqModel, index: number): OpResult<never> {
  const own = blocksOf(model)[index];
  if (own === undefined) {
    return refused("gone");
  }
  const gone = new Set(own);
  const wanted = readingOf(model);
  withoutRecords(wanted, new Set(own.flatMap((s) => s.records)));
  return checkedSeq(
    model,
    statementRemovals(model, gone).map((span) => ({ ...span, insert: "" })),
    wanted,
  );
}

function deleteParticipant(model: SeqModel, name: string): OpResult<never> {
  if (!model.participants.has(name)) {
    return refused("gone");
  }
  const gone = new Set<SeqStatement>();
  const cuts: Span[] = [];
  const wanted = readingOf(model);
  for (const statement of model.statements) {
    const actors = statement.actors;
    const here = actors.filter((actor) => actor.name === name);
    if (here.length === 0) {
      continue;
    }
    const [first, second] = actors;
    const row = wanted.records[statement.records[0] ?? -1];
    if (
      statement.kind === "note" &&
      here.length === 1 &&
      first !== undefined &&
      second !== undefined &&
      row !== undefined
    ) {
      // The name and one comma, whatever space stands around them.
      const kept = first.name === name ? second : first;
      cuts.push(
        first.name === name
          ? { from: first.span.from, to: second.span.from }
          : { from: first.span.to, to: second.span.to },
      );
      row.from = kept.name;
      row.to = kept.name;
    } else {
      gone.add(statement);
    }
  }
  if (emptiesSection(model, gone)) {
    return refused("emptiesBlock");
  }
  if (raisesActivations(model, gone)) {
    return refused("leavesActive");
  }
  withoutRecords(wanted, new Set([...gone].flatMap((s) => s.records)));
  wanted.people.delete(name);
  wanted.boxes = wanted.boxes.map((box) => box.filter((n) => n !== name));
  wanted.created.delete(name);
  wanted.destroyed.delete(name);
  return checkedRemoval(
    model,
    gone,
    [...statementRemovals(model, gone), ...cuts],
    wanted,
    name,
  );
}

/**
 * The edits for `op`, each checked against what the body must read as afterwards.
 * `created` is the new participant's or message's target. `{ edits: [] }` means the
 * source already says this.
 *
 * Refusal codes: `gone` for a name, target or block not there; `emptiesBlock` for a
 * delete that would leave a block section empty (R56), which `removeBlock` then opens;
 * `leavesActive` for a delete that would leave more activations open; `unsupported`
 * for a body that is not understood and a rename of a name with a space (R61);
 * `invalid` for all the rest.
 */
export function compileSeqOp(model: SeqModel, op: SeqOp): OpResult<SeqCreated> {
  if (!model.understood) {
    return refused("unsupported");
  }
  switch (op.op) {
    case "renameParticipant":
      return renameParticipant(model, op.name, op.text);
    case "renameMessage":
      return renameMessage(model, op.target, op.text);
    case "addParticipant":
      return addParticipant(model);
    case "addMessage":
      return addMessage(model, op.from, op.to);
    case "arrowStyle":
      return arrowStyle(model, op.target, op.line, op.head);
    case "deleteMessage":
      return deleteMessage(model, op.target);
    case "deleteParticipant":
      return deleteParticipant(model, op.name);
    case "removeBlock":
      return removeBlock(model, op.block);
  }
}

/** How many messages say what `target` says, its occurrence aside. */
function equalMessages(model: SeqModel, target: MessageTarget): number {
  return model.messages.filter(
    (m) =>
      m.from === target.from &&
      m.to === target.to &&
      textOf(model, m) === target.text,
  ).length;
}

/** A block as its own lines read: two blocks in the same words are the same. */
function blockWords(model: SeqModel, block: readonly SeqStatement[]): string {
  return JSON.stringify(
    block.map((s) => model.body.slice(s.span.from, s.span.to)),
  );
}

/**
 * `op` as an `OpResolver` for `runDiagramOp`: the op that is meant on each read of
 * the fence, or `stale` where that cannot be told (R94). A message is named by its
 * occurrence and a block by its number, and a peer's edit moves both, so on a body
 * that is no longer the one read first (`read`, or else the first pass) such an op
 * holds only for a message with no equal in either reading, or a block with no
 * other in the same words: it is then that one wherever it went. Otherwise
 * `stale`, which is true: a retry reads what stands now. A message whose pair has
 * none left is `gone`. An op naming participants by name, or naming nothing,
 * stands as given.
 *
 * Callers pass this, never a plain op with a message or a block in it: the apply
 * layer compiles a plain op again on a changed body as it is.
 */
export function heldSeqOp(
  op: SeqOp,
  read?: SeqModel,
): (model: SeqModel) => SeqOp | { refused: RefusalCode } {
  let first = read;
  return (model) => {
    first ??= model;
    if (model.body === first.body) {
      return op;
    }
    switch (op.op) {
      case "renameMessage":
      case "arrowStyle":
      case "deleteMessage": {
        const { target } = op;
        if (equalMessages(model, target) === 0) {
          const pair = model.messages.some(
            (m) => m.from === target.from && m.to === target.to,
          );
          return { refused: pair ? "stale" : "gone" };
        }
        if (
          equalMessages(first, target) !== 1 ||
          equalMessages(model, target) !== 1
        ) {
          return { refused: "stale" };
        }
        return op;
      }
      case "removeBlock": {
        const was = blocksOf(first);
        const own = was[op.block];
        if (own === undefined) {
          return { refused: "gone" };
        }
        const words = blockWords(first, own);
        const same = (m: SeqModel, blocks: SeqStatement[][]) =>
          blocks.flatMap((b, i) => (blockWords(m, b) === words ? [i] : []));
        const now = same(model, blocksOf(model));
        if (same(first, was).length !== 1 || now.length !== 1) {
          return { refused: "stale" };
        }
        return { ...op, block: now[0] as number };
      }
      default:
        return op;
    }
  };
}
