/**
 * Sequence ops compiled to minimal text edits: an op changes only the bytes it
 * means, every other line stays as written, and when that cannot be promised it
 * refuses.
 *
 * Every compile checks itself before it answers (`checkedSeq`): the edits are
 * applied, the result is parsed again, and it must be understood and read exactly
 * as the op says: the order of the participants, their labels and types, each
 * record, who is created and destroyed, and each box. That is what refuses a
 * delete that would move or lose an undeclared participant, or take the message a
 * `create` waits for. The apply layer then asks mermaid's db as well, which is the
 * only layer that sees an activation left unbalanced: the parser does not count
 * them.
 *
 * Spans are read from `model.body`, the string they point into.
 */

import type { OpResult, RefusalCode, Span, TextEdit } from "../edits";
import { applyEdits } from "../edits";
import { lineDeletions } from "../flowchart/linkStyle";
import {
  drawsOtherwise,
  encodeConfigAlias,
  encodeSeqText,
  tidySeqText,
} from "./labels";
import {
  ARROW_TYPES,
  type Declaration,
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
  | { op: "deleteParticipant"; name: string };

const NEW_PARTICIPANT = "New participant";
const NEW_MESSAGE = "New message";
const FALLBACK_INDENT = "    ";

const refused = (code: RefusalCode): OpResult => ({ refused: code });

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
interface Reading {
  order: string[];
  /** Per participant, in `order`: its label and type. */
  people: Map<string, { label: string; type: string }>;
  records: RecordRow[];
  boxes: string[][];
  created: Map<string, number>;
  destroyed: Map<string, number>;
}

function readingOf(model: SeqModel): Reading {
  return {
    order: [...model.order],
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

function sameReading(a: Reading, b: Reading): boolean {
  const flat = (r: Reading): string =>
    JSON.stringify([
      r.order,
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
function checkedSeq(
  model: SeqModel,
  edits: TextEdit[],
  wanted: Reading,
  created?: SeqTarget,
): OpResult {
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
): OpResult {
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

/** The statement of a participant's first mention, when no declaration came first. */
function firstMention(model: SeqModel, name: string): SeqStatement | undefined {
  const line = model.participants.get(name)?.firstLine;
  return model.statements.find(
    (s) =>
      s.line === line &&
      s.kind !== "activate" &&
      s.kind !== "deactivate" &&
      s.kind !== "destroy" &&
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

function renameParticipant(
  model: SeqModel,
  name: string,
  typed: string,
): OpResult {
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
  if (declared) {
    const after = declared.config?.to ?? declared.nameSpan.to;
    return checkedSeq(
      model,
      [{ from: after, to: after, insert: ` as ${encoded}` }],
      wanted,
    );
  }
  const mention = firstMention(model, name);
  if (mention === undefined) {
    return refused("invalid");
  }
  const line = `participant ${name} as ${encoded}`;
  const holder = outermost(model, mention);
  const start = model.scan.lines[holder.line]?.from;
  if (start !== undefined) {
    const before = checkedSeq(
      model,
      [
        {
          from: start,
          to: start,
          insert: `${indentOf(model, holder.line)}${line}\n`,
        },
      ],
      wanted,
    );
    if (!("refused" in before)) {
      return before;
    }
  }
  // Up there it would go left of a participant first mentioned after that point, or
  // come between a `create` or `destroy` and its message. A declaration after the
  // mention only sets the label, and the order stays as it was.
  const after = newLineAfter(
    model,
    mention.line,
    `${indentOf(model, mention.line)}${line}`,
  );
  return after === null
    ? refused("invalid")
    : checkedSeq(model, [after], wanted);
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
    const p = model.participants.get(name);
    const first = p?.declarations[0];
    return (
      first !== undefined &&
      !first.create &&
      first.line === p?.firstLine &&
      first.line < firstMessage
    );
  });
  const statements = model.statements;
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

function addParticipant(model: SeqModel): OpResult {
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
  wanted.order.push(name);
  wanted.people.set(name, { label: NEW_PARTICIPANT, type: "participant" });
  return checkedSeq(model, [edit], wanted, { kind: "participant", name });
}

function addMessage(model: SeqModel, from: string, to: string): OpResult {
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
): OpResult {
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
    for (let i = 0; i < statements.length; i++) {
      const first = statements[i];
      if (first === undefined || !gone.has(first)) {
        continue;
      }
      let end = i;
      while (gone.has(statements[end + 1] as SeqStatement)) {
        end++;
      }
      const last = statements[end] ?? first;
      const next = statements[end + 1];
      const previous = statements[i - 1];
      parts.push(
        next
          ? { from: first.span.from, to: next.span.from }
          : previous
            ? { from: previous.span.to, to: last.span.to }
            : { from: first.span.from, to: last.span.to },
      );
      i = end;
    }
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

function deleteMessage(model: SeqModel, target: MessageTarget): OpResult {
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
  const wanted = readingOf(model);
  withoutRecords(wanted, new Set(statement.records));
  return checkedSeq(
    model,
    statementRemovals(model, gone).map((span) => ({ ...span, insert: "" })),
    wanted,
  );
}

function deleteParticipant(model: SeqModel, name: string): OpResult {
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
  withoutRecords(wanted, new Set([...gone].flatMap((s) => s.records)));
  wanted.order = wanted.order.filter((n) => n !== name);
  wanted.people.delete(name);
  wanted.boxes = wanted.boxes.map((box) => box.filter((n) => n !== name));
  wanted.created.delete(name);
  wanted.destroyed.delete(name);
  return checkedSeq(
    model,
    [...statementRemovals(model, gone), ...cuts].map((span) => ({
      ...span,
      insert: "",
    })),
    wanted,
  );
}

/**
 * The edits for `op`, each checked against what the body must read as afterwards.
 * `created` is the new participant's or message's target. `{ edits: [] }` means the
 * source already says this.
 *
 * Refusal codes: `gone` for a name or target not there; `emptiesBlock` for a delete
 * that would leave a block section empty (R56); `unsupported` for a body that is not
 * understood and a rename of a name with a space (R61); `invalid` for all the rest.
 */
export function compileSeqOp(model: SeqModel, op: SeqOp): OpResult {
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
  }
}
