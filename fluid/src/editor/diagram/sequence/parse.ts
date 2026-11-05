/**
 * The sequence parser: reads a fence body into a `SeqModel` that keeps the
 * source span of every name, arrow and text.
 *
 * This grammar does not fit the flowchart parser's way of working. Its lexer
 * is case-insensitive, takes the first rule that matches and not the longest,
 * ends a statement at any `;` whatever surrounds it, and has rules that look
 * past a line break. So the lexer below is mermaid 12.2.0's own: its rule table
 * and start conditions copied as they are (`LEXER_RULES`, `LEXER_CONDITIONS`;
 * `agreement.test.ts` compares them with the real ones and runs the real lexer
 * beside this one over the corpus), on the text mermaid's preprocessing would
 * give it (`lexedText`), with each offset mapped back to the body. The grammar
 * is walked by hand with one explicit stack, statement by statement, in
 * the order the db's `apply` runs. See
 * research/2026-11-05-mermaid-12.2-sequence-ids.md.
 *
 * It knows a deliberate subset. Anything outside it leaves `understood` false
 * with a reason, and the fence keeps its text:
 * - what cannot be read as mermaid reads it: a syntax error, an unclosed block;
 * - what the model cannot hold: half arrows and `()` central connections;
 * - a statement run over lines, a body or a line past the limits;
 * - a `#` that cuts a text or takes all of it (`A->>B: Issue #12`, `rect #fff`),
 *   a `;` that `encodeEntities` removes, a `%%{` inside
 *   a line, a lone CR: the source and the drawing would say different things,
 *   and nobody could see why with the text hidden;
 * - a name with `#`, `%` or an entity;
 * - `hideUnusedParticipants` named in the frontmatter or a directive: the db still
 *   lists the participants it hides, so they would have no element;
 * - what the db accepts and the render cannot draw or shows otherwise: `activate`
 *   of nobody, a `create` or `destroy` with no message right after it, an
 *   unknown `@{ type }`, a menu on a participant drawn without a box, `$$`
 *   formulas (no `.messageText`, a separate renderer).
 *
 * It never throws: a body it trips over comes back not understood.
 */

import { entityText } from "../../../theme/mermaidText";
import type { Span } from "../edits";
import { MAX_TEXT_SIZE } from "../flowchart/parse";
import { scanFence } from "../scan";
import type {
  Declaration,
  SeqArrow,
  SeqHead,
  SeqMessage,
  SeqModel,
  SeqRecord,
  SeqStatement,
  SeqStatementKind,
} from "./model";

/**
 * The longest line this parser reads. Rule 13 is quadratic on a run of spaces
 * that does not end a name, and the verdict runs on every changed body, so the
 * cost has to stay bounded per line. A real line is far below it.
 */
export const MAX_SEQ_LINE = 2000;

/** mermaid's lexer rules by number, each without its `^(?:` and `)`. */
export const LEXER_RULES: readonly string[] = [
  String.raw`[\n]+`,
  String.raw`\s+`,
  String.raw`((?!\n)\s)+`,
  String.raw`#[^\n]*`,
  String.raw`%(?!\{)[^\n]*`,
  String.raw`[^\}]%%[^\n]*`,
  String.raw`([0-9]+(\.[0-9]{1,2})?|\.[0-9]{1,2})(?=[ \n]+)`,
  String.raw`@\{`,
  String.raw`[^\}]+`,
  String.raw`\}(?=\s+as\s)`,
  String.raw`\}`,
  String.raw`[^<>:\n,;@\s]+(?=\s*@\{)`,
  String.raw`[^<>:\n,;@\s]+(?=\s+as\s)`,
  String.raw`[^<>:\n,;@]+(?=\s*[\n;#]|$)`,
  String.raw`[^<>:\n,;@]*<[^\n]*`,
  String.raw`[^\n]+`,
  String.raw`box\b`,
  String.raw`participant\b`,
  String.raw`actor\b`,
  String.raw`create\b`,
  String.raw`destroy\b`,
  String.raw`as\b`,
  String.raw`(?:)`,
  String.raw`loop\b`,
  String.raw`rect\b`,
  String.raw`opt\b`,
  String.raw`alt\b`,
  String.raw`else\b`,
  String.raw`par\b`,
  String.raw`par_over\b`,
  String.raw`and\b`,
  String.raw`critical\b`,
  String.raw`option\b`,
  String.raw`break\b`,
  String.raw`(?:[:]?(?:no)?wrap)?[^#\n;]*`,
  String.raw`end\b`,
  String.raw`left of\b`,
  String.raw`right of\b`,
  String.raw`links(?=[ \t]+[^\/\\\+\(\)<>:\n,;\s\-])`,
  String.raw`link(?=[ \t]+[^\/\\\+\(\)<>:\n,;\s\-])`,
  String.raw`properties(?=[ \t]+[^\/\\\+\(\)<>:\n,;\s\-])`,
  String.raw`details(?=[ \t]+[^\/\\\+\(\)<>:\n,;\s\-])`,
  String.raw`over\b`,
  String.raw`note\b`,
  String.raw`activate\b`,
  String.raw`deactivate\b`,
  String.raw`title\s[^#\n;]+`,
  String.raw`title:\s[^#\n;]+`,
  String.raw`accTitle\s*:\s*`,
  String.raw`(?!\n||)*[^\n]*`,
  String.raw`accDescr\s*:\s*`,
  String.raw`(?!\n||)*[^\n]*`,
  String.raw`accDescr\s*\{\s*`,
  String.raw`[\}]`,
  String.raw`[^\}]*`,
  String.raw`sequenceDiagram\b`,
  String.raw`autonumber\b`,
  String.raw`off\b`,
  String.raw`,`,
  String.raw`;`,
  String.raw`[^\/\\\+\()\+<\->\->:\n,;]+((?!(-x|--x|-\)|--\)|-\|\\|-\\|-\/|-\/\/|-\|\/|\/\|-|\\\|-|\/\/-|\\\\-|\/\|-|--\|\\|--|\(\)))[\-]*[^\+<\->\->:\n,;]+)*`,
  String.raw`->>`,
  String.raw`<<->>`,
  String.raw`-->>`,
  String.raw`<<-->>`,
  String.raw`->`,
  String.raw`-->`,
  String.raw`-[x]`,
  String.raw`--[x]`,
  String.raw`-[\)]`,
  String.raw`--[\)]`,
  String.raw`--\|\\`,
  String.raw`--\|\/`,
  String.raw`--\\\\`,
  String.raw`--\/\/`,
  String.raw`\/\|--`,
  String.raw`\\\|--`,
  String.raw`\/\/--`,
  String.raw`\\\\--`,
  String.raw`-\|\\`,
  String.raw`-\|\/`,
  String.raw`-\\\\`,
  String.raw`-\/\/`,
  String.raw`\/\|-`,
  String.raw`\\\|-`,
  String.raw`\/\/-`,
  String.raw`\\\\-`,
  String.raw`:(?:(?:no)?wrap)?[^#\n;]*`,
  String.raw`:`,
  String.raw`\+`,
  String.raw`-`,
  String.raw`\(\)`,
  String.raw`$`,
  String.raw`.`,
];

/** mermaid's start conditions: the rules each may try, in order. */
export const LEXER_CONDITIONS: Readonly<Record<string, readonly number[]>> = {
  acc_descr_multiline: [53, 54],
  acc_descr: [51],
  acc_title: [49],
  ID: [2, 3, 7, 11, 12, 13, 14, 15],
  ALIAS: [2, 3, 21, 22],
  LINE: [2, 3, 34],
  CONFIG: [8, 9, 10],
  CONFIG_DATA: [],
  INITIAL: [
    0, 1, 3, 4, 5, 6, 16, 17, 18, 19, 20, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    32, 33, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 50, 52, 55,
    56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74,
    75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93,
  ],
};

/** What each rule does: its token, and how it moves the condition stack. */
const RULE_ACTIONS: Readonly<
  Record<number, { token?: string; pop?: number; begin?: string }>
> = {
  0: { token: "NEWLINE" },
  6: { token: "NUM" },
  7: { token: "CONFIG_START", begin: "CONFIG" },
  8: { token: "CONFIG_CONTENT" },
  9: { token: "CONFIG_END", pop: 1, begin: "ALIAS" },
  10: { token: "CONFIG_END", pop: 2 },
  11: { token: "ACTOR" },
  12: { token: "ACTOR", begin: "ALIAS" },
  13: { token: "ACTOR", pop: 1 },
  14: { token: "INVALID", pop: 1 },
  15: { token: "INVALID", pop: 1 },
  16: { token: "box", begin: "LINE" },
  17: { token: "participant", begin: "ID" },
  18: { token: "participant_actor", begin: "ID" },
  19: { token: "create" },
  20: { token: "destroy", begin: "ID" },
  21: { token: "AS", pop: 2, begin: "LINE" },
  22: { token: "NEWLINE", pop: 2 },
  23: { token: "loop", begin: "LINE" },
  24: { token: "rect", begin: "LINE" },
  25: { token: "opt", begin: "LINE" },
  26: { token: "alt", begin: "LINE" },
  27: { token: "else", begin: "LINE" },
  28: { token: "par", begin: "LINE" },
  29: { token: "par_over", begin: "LINE" },
  30: { token: "and", begin: "LINE" },
  31: { token: "critical", begin: "LINE" },
  32: { token: "option", begin: "LINE" },
  33: { token: "break", begin: "LINE" },
  34: { token: "restOfLine", pop: 1 },
  35: { token: "end" },
  36: { token: "left_of" },
  37: { token: "right_of" },
  38: { token: "links" },
  39: { token: "link" },
  40: { token: "properties" },
  41: { token: "details" },
  42: { token: "over" },
  43: { token: "note" },
  44: { token: "activate", begin: "ID" },
  45: { token: "deactivate", begin: "ID" },
  46: { token: "title" },
  47: { token: "legacy_title" },
  48: { token: "acc_title", begin: "acc_title" },
  49: { token: "acc_title_value", pop: 1 },
  50: { token: "acc_descr", begin: "acc_descr" },
  51: { token: "acc_descr_value", pop: 1 },
  52: { begin: "acc_descr_multiline" },
  53: { pop: 1 },
  54: { token: "acc_descr_multiline_value" },
  55: { token: "SD" },
  56: { token: "autonumber" },
  57: { token: "off" },
  58: { token: "," },
  59: { token: "NEWLINE" },
  60: { token: "ACTOR" },
  61: { token: "SOLID_ARROW" },
  62: { token: "BIDIRECTIONAL_SOLID_ARROW" },
  63: { token: "DOTTED_ARROW" },
  64: { token: "BIDIRECTIONAL_DOTTED_ARROW" },
  65: { token: "SOLID_OPEN_ARROW" },
  66: { token: "DOTTED_OPEN_ARROW" },
  67: { token: "SOLID_CROSS" },
  68: { token: "DOTTED_CROSS" },
  69: { token: "SOLID_POINT" },
  70: { token: "DOTTED_POINT" },
  71: { token: "SOLID_ARROW_TOP_DOTTED" },
  72: { token: "SOLID_ARROW_BOTTOM_DOTTED" },
  73: { token: "STICK_ARROW_TOP_DOTTED" },
  74: { token: "STICK_ARROW_BOTTOM_DOTTED" },
  75: { token: "SOLID_ARROW_TOP_REVERSE_DOTTED" },
  76: { token: "SOLID_ARROW_BOTTOM_REVERSE_DOTTED" },
  77: { token: "STICK_ARROW_TOP_REVERSE_DOTTED" },
  78: { token: "STICK_ARROW_BOTTOM_REVERSE_DOTTED" },
  79: { token: "SOLID_ARROW_TOP" },
  80: { token: "SOLID_ARROW_BOTTOM" },
  81: { token: "STICK_ARROW_TOP" },
  82: { token: "STICK_ARROW_BOTTOM" },
  83: { token: "SOLID_ARROW_TOP_REVERSE" },
  84: { token: "SOLID_ARROW_BOTTOM_REVERSE" },
  85: { token: "STICK_ARROW_TOP_REVERSE" },
  86: { token: "STICK_ARROW_BOTTOM_REVERSE" },
  87: { token: "TXT" },
  88: { token: "TXT" },
  89: { token: "+" },
  90: { token: "-" },
  91: { token: "()" },
  92: { token: "NEWLINE" },
  93: { token: "INVALID" },
};

/** Rules whose token mermaid trims; the rest keep their outer space. */
const TRIMMED_RULES = new Set([11, 12, 13, 15, 60]);

const ARROWS: Readonly<
  Record<string, { line: SeqArrow["line"]; head: SeqHead; both: boolean }>
> = {
  SOLID_OPEN_ARROW: { line: "solid", head: "open", both: false },
  DOTTED_OPEN_ARROW: { line: "dotted", head: "open", both: false },
  SOLID_ARROW: { line: "solid", head: "arrow", both: false },
  DOTTED_ARROW: { line: "dotted", head: "arrow", both: false },
  BIDIRECTIONAL_SOLID_ARROW: { line: "solid", head: "arrow", both: true },
  BIDIRECTIONAL_DOTTED_ARROW: { line: "dotted", head: "arrow", both: true },
  SOLID_CROSS: { line: "solid", head: "cross", both: false },
  DOTTED_CROSS: { line: "dotted", head: "cross", both: false },
  SOLID_POINT: { line: "solid", head: "async", both: false },
  DOTTED_POINT: { line: "dotted", head: "async", both: false },
};

/** mermaid's `LINETYPE` for each arrow token. */
const ARROW_TOKEN_TYPES: Readonly<Record<string, number>> = {
  SOLID_ARROW: 0,
  DOTTED_ARROW: 1,
  SOLID_CROSS: 3,
  DOTTED_CROSS: 4,
  SOLID_OPEN_ARROW: 5,
  DOTTED_OPEN_ARROW: 6,
  SOLID_POINT: 24,
  DOTTED_POINT: 25,
  BIDIRECTIONAL_SOLID_ARROW: 33,
  BIDIRECTIONAL_DOTTED_ARROW: 34,
};

const ACTIVE_START = 17;
const ACTIVE_END = 18;
const NOTE = 2;
const AUTONUMBER = 26;

/** Each block keyword's kind and its records in mermaid's `LINETYPE`. */
const BLOCKS: Readonly<
  Record<
    string,
    { start: number; end: number; middle?: string; midType?: number }
  >
> = {
  loop: { start: 10, end: 11 },
  rect: { start: 22, end: 23 },
  opt: { start: 15, end: 16 },
  alt: { start: 12, end: 14, middle: "else", midType: 13 },
  par: { start: 19, end: 21, middle: "and", midType: 20 },
  par_over: { start: 32, end: 21, middle: "and", midType: 20 },
  critical: { start: 27, end: 29, middle: "option", midType: 28 },
  break: { start: 30, end: 31 },
};

const LEFT_OF = 0;
const RIGHT_OF = 1;
const OVER = 2;

const LINK_TOKENS = new Set(["links", "link", "properties", "details"]);

const HALF_ARROW = /^(?:SOLID|STICK)_ARROW_(?:TOP|BOTTOM)/;

/**
 * The types whose drawing has no box for a menu: `drawPopup` throws on a
 * `link`, `links` or `details` line of one of them, though the db takes it.
 */
const TYPES_WITHOUT_MENU = new Set(["actor", "boundary", "control", "entity"]);

/** The `@{ type }` values mermaid draws; another value draws nothing at all. */
const PARTICIPANT_TYPES = new Set([
  "actor",
  "boundary",
  "collections",
  "control",
  "database",
  "entity",
  "participant",
  "queue",
]);

/**
 * The tokens after which a rule 3 comment on the same line loses text: a text
 * that the `#` cuts, and a keyword that opens a `LINE`, whose whole text it takes.
 */
const CUT_BY_A_HASH = new Set([
  "TXT",
  "restOfLine",
  "title",
  "legacy_title",
  "box",
  "loop",
  "rect",
  "opt",
  "alt",
  "else",
  "par",
  "par_over",
  "and",
  "critical",
  "option",
  "break",
]);

const COMPILED = LEXER_RULES.map((source) => new RegExp(source, "iy"));

/** A failure inside one statement: the message for `reasons`. */
class Refusal extends Error {}

function refuse(message: string): never {
  throw new Refusal(message);
}

/** One token, in `lexedText` offsets. An ACTOR's offsets are its trimmed text. */
export interface SeqToken {
  kind: string;
  from: number;
  to: number;
}

/** What mermaid's lexer gets, and where each of its characters sits in the body. */
interface Lexed {
  text: string;
  /** The body offset of each character of `text`. */
  map: Int32Array;
}

/**
 * Whether `encodeEntities` removes a `;` from `line`, that is whether it
 * matches `/style.*:\S*#.*;/` or `/classDef.*:\S*#.*;/`. Those regexes backtrack
 * cubically, and the scanner has run them once already; this is one pass.
 */
export function losesSemicolon(line: string): boolean {
  const last = line.lastIndexOf(";");
  return (
    last !== -1 &&
    (holdsEntityRule(line, "style", last) ||
      holdsEntityRule(line, "classDef", last))
  );
}

function holdsEntityRule(line: string, word: string, last: number): boolean {
  const start = line.indexOf(word);
  if (start === -1) {
    return false;
  }
  let colon = false;
  for (let i = start + word.length; i < last; i++) {
    const c = line.charAt(i);
    if (c === ":") {
      colon = true;
    } else if (/\s/.test(c)) {
      colon = false;
    } else if (c === "#" && colon) {
      return true;
    }
  }
  return false;
}

/** A `#name;` the way `encodeEntities` hides it: a same-length run of ordinary characters. */
function maskEntities(line: string): string {
  return line.replace(/#\w+;/g, (entity) => "ﬂ".repeat(entity.length));
}

/**
 * The text mermaid's lexer reads, for an understood scan: without the lines
 * its preprocessing removes (frontmatter, directives, whole-line comments),
 * with CRLF as LF, with entities hidden and with one last line break.
 * `encodeEntities` removing a `;` is refused here and not mirrored.
 */
function lexedText(
  model: Pick<SeqModel, "body" | "scan">,
  reasons: string[],
): Lexed {
  const { body, scan } = model;
  const text: string[] = [];
  const map = new Int32Array(body.length + 2);
  let length = 0;
  for (const line of scan.lines) {
    if (
      line.kind === "frontmatter" ||
      line.kind === "directive" ||
      line.kind === "comment"
    ) {
      if (
        line.kind !== "comment" &&
        body.slice(line.from, line.to).includes("hideUnusedParticipants")
      ) {
        reasons.push(
          "hideUnusedParticipants leaves participants without an element.",
        );
      }
      continue;
    }
    let raw = body.slice(line.from, line.to);
    if (raw.endsWith("\r")) {
      raw = raw.slice(0, -1);
    }
    if (line.kind === "header" || line.kind === "statement") {
      if (raw.length > MAX_SEQ_LINE) {
        reasons.push(`Line ${String(line.index + 1)} is too long to read.`);
        continue;
      }
      if (losesSemicolon(raw)) {
        reasons.push(
          `mermaid removes a ; from line ${String(line.index + 1)}, so what follows joins the line.`,
        );
      }
    } else if (raw.length > MAX_SEQ_LINE) {
      reasons.push(`Line ${String(line.index + 1)} is too long to read.`);
      continue;
    }
    if (raw.includes("%%{")) {
      reasons.push(
        `Line ${String(line.index + 1)} holds a directive inside the line.`,
      );
    }
    text.push(maskEntities(raw), "\n");
    for (let i = 0; i <= raw.length; i++) {
      map[length + i] = line.from + i;
    }
    length += raw.length + 1;
  }
  map[length] = body.length;
  return { text: text.join(""), map };
}

/**
 * mermaid's lexer over `text`. A comment that cuts a text (rule 3 right after
 * one) and a rule 5 comment, which hides a statement's start, are refused here,
 * so a clean run holds no such thing.
 */
function lex(text: string, reasons: string[]): SeqToken[] {
  const tokens: SeqToken[] = [];
  const stack = ["INITIAL"];
  const length = text.length;
  // `cleanupComments` ends in `trimStart`.
  let at = length - text.trimStart().length;
  let done = false;
  while (!done) {
    if (at === length) {
      done = true;
    }
    const rules = LEXER_CONDITIONS[stack[stack.length - 1] ?? "INITIAL"] ?? [];
    let rule = -1;
    let end = at;
    for (const candidate of rules) {
      const re = COMPILED[candidate];
      if (re === undefined) {
        continue;
      }
      re.lastIndex = at;
      if (re.test(text)) {
        rule = candidate;
        end = re.lastIndex;
        break;
      }
    }
    if (rule === -1) {
      if (at === length) {
        break;
      }
      reasons.push("mermaid's lexer cannot read a line here.");
      break;
    }
    if (rule === 3) {
      const last = tokens[tokens.length - 1]?.kind;
      if (last !== undefined && CUT_BY_A_HASH.has(last)) {
        reasons.push(
          "A # starts a comment here, and mermaid drops the rest of the text.",
        );
      }
    } else if (rule === 5) {
      reasons.push("A %% hides a statement here.");
    }
    const action = RULE_ACTIONS[rule];
    if (action?.pop !== undefined) {
      stack.length -= action.pop;
    }
    if (action?.begin !== undefined) {
      stack.push(action.begin);
    }
    if (action?.token !== undefined) {
      let from = at;
      let to = end;
      if (TRIMMED_RULES.has(rule)) {
        while (from < to && /\s/.test(text.charAt(from))) {
          from++;
        }
        while (to > from && /\s/.test(text.charAt(to - 1))) {
          to--;
        }
      }
      tokens.push({ kind: action.token, from, to });
    }
    at = end;
  }
  return tokens;
}

/** A body's preprocessing and tokens, for the test that runs mermaid's lexer beside it. */
export function lexSequence(body: string): SeqToken[] {
  const reasons: string[] = [];
  const lexed = lexedText({ body, scan: scanFence(body) }, reasons);
  return lex(lexed.text, reasons);
}

/**
 * A sequence text as drawn: each `<br>` form a line break (mermaid's
 * `lineBreakRegex`, `</br>` too), then entities decoded, outer space gone. The
 * order matters: `a#lt;br#gt;b` draws the text `a<br>b`. mermaid's own renderer
 * decodes these forms in sequence diagrams (`drawnLabels` finds nothing left to
 * do there).
 *
 * A form that names no character keeps its source form here, where the drawing
 * differs: `#foo;` and `#fff;` draw `&foo;` and `&fff;`, and `#0;`, `#1114112;`
 * and `#55296;` draw U+FFFD. Nothing looks a message up by its drawn text, so
 * `SeqTarget.text` and `occurrence` stay sound, but a field filled from this
 * shows the source form.
 */
export function decodeSeqText(raw: string): string {
  return raw
    .replace(/<\/?br\s*\/?>/gi, "\n")
    .replace(/#(\d+|\w+);/g, (form, name: string) => {
      const key = /^\d+$/.test(name) ? `#${name}` : name;
      return entityText(key) ?? form;
    })
    .trim();
}

const blank = (c: string): boolean => /\s/.test(c);

export function parseSequence(body: string): SeqModel {
  const model: SeqModel = {
    kind: "sequence",
    body,
    participants: new Map(),
    order: [],
    messages: [],
    statements: [],
    records: [],
    boxes: [],
    created: new Map(),
    destroyed: new Map(),
    scan: {
      lines: [],
      header: null,
      keyword: null,
      understood: false,
      reasons: [],
    },
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
    if (scan.keyword !== "sequenceDiagram") {
      model.reasons.push("This is not a sequence diagram.");
    } else {
      model.reasons.push(...scan.reasons);
      if (/\r(?!\n)|[\u2028\u2029]/.test(body)) {
        model.reasons.push("A line break here is one only mermaid sees.");
      }
      if (model.reasons.length === 0) {
        new Parser(model).run();
      }
    }
  } catch (error) {
    if (error instanceof Refusal) {
      model.reasons.push(error.message);
    } else {
      model.reasons.push(
        `The parser stopped: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }
  model.understood = model.reasons.length === 0;
  return model;
}

/** The `@{ }` content this parser reads: `type` and `alias`, on one line. */
const CONFIG_ENTRY =
  /\s*("?)(type|alias)\1\s*:\s*(?:"([^"\\\n]*)"|([A-Za-z][\w -]*))\s*(?:,|$)/y;

const YAML_WORD = /^(?:true|false|null|yes|no|on|off|~)$/i;

interface OpenFrame {
  /** A key of `BLOCKS`, or `box`. */
  kind: string;
  box: { actors: string[] } | null;
}

class Parser {
  private readonly tokens: SeqToken[];
  private readonly text: string;
  private readonly map: Int32Array;
  private at = 0;
  private readonly open: OpenFrame[] = [];
  /** A `create` or `destroy` that waits for its message as the next statement. */
  private waiting: { name: string; create: boolean } | null = null;
  private readonly activated: string[] = [];
  /** Who has a `link`, `links` or `details` line. */
  private readonly menus: string[] = [];
  private readonly occurrences = new Map<string, number>();

  private readonly model: SeqModel;

  constructor(model: SeqModel) {
    this.model = model;
    const reasons: string[] = [];
    const lexed = lexedText(model, reasons);
    this.text = lexed.text;
    this.map = lexed.map;
    // Any reason leaves the line kinds or the text unlike mermaid's, so the
    // tokens would be worth nothing.
    this.tokens = reasons.length === 0 ? lex(lexed.text, reasons) : [];
    model.reasons.push(...reasons);
  }

  run(): void {
    if (this.model.reasons.length > 0) {
      return;
    }
    this.skipLines();
    if (this.peek()?.kind !== "SD") {
      refuse("The diagram does not start with sequenceDiagram.");
    }
    this.at++;
    while (this.peek() !== undefined) {
      this.statement();
    }
    const open = this.open[this.open.length - 1];
    if (open !== undefined) {
      refuse(`A ${open.kind} is not closed.`);
    }
    if (this.waiting !== null) {
      this.refuseWaiting(this.waiting);
    }
    for (const name of this.activated) {
      if (!this.model.participants.has(name)) {
        refuse(
          `Nobody named ${name} is there to activate, and mermaid cannot draw that.`,
        );
      }
    }
    for (const name of this.menus) {
      const type = this.model.participants.get(name)?.type ?? "";
      if (TYPES_WITHOUT_MENU.has(type)) {
        refuse(
          `mermaid cannot draw the menu of ${name}, which is of type ${type}.`,
        );
      }
    }
  }

  private peek(ahead = 0): SeqToken | undefined {
    return this.tokens[this.at + ahead];
  }

  private skipLines(): void {
    while (this.peek()?.kind === "NEWLINE") {
      this.at++;
    }
  }

  private take(kind: string, what: string): SeqToken {
    const token = this.peek();
    if (token?.kind !== kind) {
      refuse(`Expected ${what} on line ${this.lineText(token)}.`);
    }
    this.at++;
    return token;
  }

  private lineText(token: SeqToken | undefined): string {
    return token === undefined
      ? "the last line"
      : String(this.lineAt(token) + 1);
  }

  /** The body offset of a lexed offset. */
  private bodyAt(offset: number): number {
    return this.map[offset] ?? this.model.body.length;
  }

  /** The body span of lexed offsets; `to` stays right after its last character. */
  private span(from: number, to: number): Span {
    return {
      from: this.bodyAt(from),
      to: to > from ? this.bodyAt(to - 1) + 1 : this.bodyAt(from),
    };
  }

  private lineAt(token: SeqToken): number {
    return this.lineOfBody(this.bodyAt(token.from));
  }

  private lineOfBody(offset: number): number {
    const lines = this.model.scan.lines;
    let low = 0;
    let high = lines.length - 1;
    while (low < high) {
      const middle = Math.ceil((low + high) / 2);
      if ((lines[middle]?.from ?? 0) <= offset) {
        low = middle;
      } else {
        high = middle - 1;
      }
    }
    return low;
  }

  /**
   * The span of a text token's message, as `parseMessage` and `extractWrap`
   * cut it: no outer space, no `wrap:` or `nowrap:` in front. Null for none.
   */
  private textSpan(from: number, to: number): Span | null {
    const text = this.text;
    let start = from;
    let end = to;
    while (start < end && blank(text.charAt(start))) {
      start++;
    }
    while (end > start && blank(text.charAt(end - 1))) {
      end--;
    }
    const prefix = /^:?(?:no)?wrap:/.exec(
      text.slice(start, Math.min(end, start + 8)),
    );
    if (prefix) {
      start += prefix[0].length;
      while (start < end && blank(text.charAt(start))) {
        start++;
      }
    }
    if (start === end) {
      return null;
    }
    if (/\$\$.*\$\$/.test(text.slice(start, end))) {
      refuse(
        "A formula in a text is drawn by another renderer, with no text to click.",
      );
    }
    return this.span(start, end);
  }

  /** The text after a TXT token's colon. */
  private colonText(token: SeqToken): Span | null {
    return this.textSpan(token.from + 1, token.to);
  }

  private actorToken(what: string): {
    name: string;
    span: Span;
    token: SeqToken;
  } {
    const token = this.peek();
    if (token?.kind !== "ACTOR") {
      refuse(`Expected ${what} on line ${this.lineText(token)}.`);
    }
    this.at++;
    const name = this.model.body.slice(
      this.bodyAt(token.from),
      this.span(token.from, token.to).to,
    );
    if (/\$\$.*\$\$/.test(name)) {
      refuse(`The name ${name} would be drawn as a formula.`);
    }
    if (/[#%]/.test(name)) {
      refuse(`A # or % in the name ${name} makes it read oddly.`);
    }
    return { name, span: this.span(token.from, token.to), token };
  }

  private endOfStatement(last: SeqToken): void {
    const next = this.peek();
    if (next?.kind !== "NEWLINE") {
      refuse(`Line ${this.lineText(last)} goes on where it should end.`);
    }
    this.at++;
  }

  /** A statement read over a line break reads the source another way than it looks. */
  private sameLine(first: SeqToken, last: SeqToken): void {
    const end = this.bodyAt(Math.max(last.from, last.to - 1));
    if (this.lineAt(first) !== this.lineOfBody(end)) {
      refuse(`A statement runs over from line ${this.lineText(first)}.`);
    }
  }

  private addStatement(
    first: SeqToken,
    last: SeqToken,
    kind: SeqStatementKind,
    actors: SeqStatement["actors"],
    text: Span | null,
    overLines = false,
  ): SeqStatement {
    if (!overLines) {
      this.sameLine(first, last);
    }
    if (this.waiting !== null && kind !== "message") {
      this.refuseWaiting(this.waiting);
    }
    const statement: SeqStatement = {
      line: this.lineAt(first),
      kind,
      actors,
      records: [],
      depth: this.open.length,
      span: this.span(first.from, last.to),
      text,
    };
    this.model.statements.push(statement);
    return statement;
  }

  private refuseWaiting(waiting: { name: string; create: boolean }): never {
    refuse(
      `The next statement after ${waiting.create ? "create" : "destroy"} ${waiting.name} is not its message.`,
    );
  }

  /** Adds a record to `statement`, the newest one. */
  private addRecord(
    statement: SeqStatement,
    record: Omit<SeqRecord, "statement">,
  ): number {
    const index = this.model.records.length;
    this.model.records.push({
      ...record,
      statement: this.model.statements.length - 1,
    });
    statement.records.push(index);
    return index;
  }

  /** `SequenceDB.addActor`: a mention or a declaration. */
  private addActor(
    name: string,
    line: number,
    description: string | undefined,
    draw: string | undefined,
    config?: { type?: string; alias?: string },
  ): void {
    const type = config?.type ?? draw;
    let label = description;
    if (
      config?.alias !== undefined &&
      (label === undefined || label === name)
    ) {
      label = config.alias;
    }
    const old = this.model.participants.get(name);
    if (old && label === undefined) {
      return;
    }
    if (label === undefined || type === undefined) {
      label = name;
    }
    const text = decodeSeqText(label);
    this.open[this.open.length - 1]?.box?.actors.push(name);
    if (old) {
      old.label = text;
      old.type = type ?? "participant";
      return;
    }
    this.model.order.push(name);
    this.model.participants.set(name, {
      name,
      declarations: [],
      firstLine: line,
      label: text,
      type: type ?? "participant",
    });
  }

  private statement(): void {
    const token = this.peek();
    if (token === undefined) {
      return;
    }
    const inBox = this.open[this.open.length - 1]?.kind === "box";
    switch (token.kind) {
      case "NEWLINE":
        this.at++;
        return;
      case "INVALID":
        refuse(`mermaid drops line ${this.lineText(token)} without a word.`);
        break;
      case "participant":
      case "participant_actor":
        this.declaration(token, false);
        return;
      case "destroy":
        this.destroy(token);
        return;
      case "end":
        this.end(token);
        return;
      default:
        break;
    }
    if (inBox) {
      refuse(
        `A box holds only participants, see line ${this.lineText(token)}.`,
      );
    }
    switch (token.kind) {
      case "create":
        this.create(token);
        return;
      case "box":
        this.box(token);
        return;
      case "autonumber":
        this.autonumber(token);
        return;
      case "activate":
      case "deactivate":
        this.activation(token);
        return;
      case "note":
        this.note(token);
        return;
      case "title":
      case "legacy_title":
        this.at++;
        this.addStatement(token, token, "title", [], null);
        return;
      case "acc_title":
      case "acc_descr":
        this.acc(token);
        return;
      case "acc_descr_multiline_value":
        this.accBlock(token);
        return;
      case "ACTOR":
        this.message();
        return;
      default:
        break;
    }
    if (LINK_TOKENS.has(token.kind)) {
      this.link(token);
      return;
    }
    if (BLOCKS[token.kind] !== undefined) {
      this.block(token);
      return;
    }
    const open = this.open[this.open.length - 1];
    if (open !== undefined && BLOCKS[open.kind]?.middle === token.kind) {
      this.middle(token, open);
      return;
    }
    refuse(`mermaid does not read line ${this.lineText(token)}.`);
  }

  private declaration(
    keyword: SeqToken,
    create: boolean,
    createToken?: SeqToken,
  ): void {
    this.at++;
    const actor = this.actorToken("a participant name");
    let configToken: SeqToken | null = null;
    let config: {
      type?: string;
      alias?: string;
      aliasSpan: Span | null;
    } | null = null;
    let last = actor.token;
    if (this.peek()?.kind === "CONFIG_START") {
      const start = this.take("CONFIG_START", "@{");
      const content = this.take("CONFIG_CONTENT", "the content of @{ }");
      const close = this.take("CONFIG_END", "}");
      config = this.readConfig(content);
      configToken = { kind: "CONFIG", from: start.from, to: close.to };
      last = close;
    }
    let alias: Span | null = null;
    let description: string | undefined;
    if (this.peek()?.kind === "AS") {
      this.at++;
      const rest = this.take("restOfLine", "the text after as");
      alias = this.textSpan(rest.from, rest.to);
      if (alias === null) {
        refuse(`The alias on line ${this.lineText(rest)} is empty.`);
      }
      description = this.model.body.slice(alias.from, alias.to);
      if (config?.alias !== undefined) {
        refuse(`Line ${this.lineText(rest)} has two aliases.`);
      }
      last = rest;
    }
    this.endOfStatement(last);
    const name = actor.name;
    const first = createToken ?? keyword;
    const statement = this.addStatement(
      first,
      last,
      "declaration",
      [{ name, span: actor.span }],
      alias ?? config?.aliasSpan ?? null,
    );
    const draw = keyword.kind === "participant" ? "participant" : "actor";
    this.addActor(name, statement.line, description, draw, config ?? undefined);
    const participant = this.model.participants.get(name);
    const entry: Declaration = {
      line: statement.line,
      span: statement.span,
      keyword: draw,
      nameSpan: actor.span,
      alias: alias ?? config?.aliasSpan ?? null,
      create,
      inBox: this.open[this.open.length - 1]?.kind === "box",
      config: configToken && this.span(configToken.from, configToken.to),
    };
    participant?.declarations.push(entry);
    if (create) {
      this.model.created.set(name, this.model.records.length);
      this.waiting = { name, create: true };
    }
  }

  /** The `@{ }` this parser reads: `type` and `alias`, on one line; else not understood. */
  private readConfig(content: SeqToken): {
    type?: string;
    alias?: string;
    aliasSpan: Span | null;
  } {
    const raw = this.text.slice(content.from, content.to);
    const line = this.lineText(content);
    if (raw.includes("\n")) {
      refuse(`The @{ } on line ${line} runs over lines.`);
    }
    const out: { type?: string; alias?: string; aliasSpan: Span | null } = {
      aliasSpan: null,
    };
    const entry = new RegExp(CONFIG_ENTRY.source, "y");
    entry.lastIndex = 0;
    const trimmed = raw.trimEnd();
    if (trimmed.trim() === "") {
      refuse(`The @{ } on line ${line} is empty.`);
    }
    while (entry.lastIndex < trimmed.length) {
      const start = entry.lastIndex;
      const match = entry.exec(trimmed);
      if (match === null) {
        refuse(`The @{ } on line ${line} holds more than this parser reads.`);
      }
      const value = match[3] ?? match[4] ?? "";
      if (match[3] === undefined && YAML_WORD.test(value.trim())) {
        refuse(`The @{ } on line ${line} holds more than this parser reads.`);
      }
      const key = match[2];
      if (key === "type") {
        // A quoted value keeps its spaces in the YAML, a bare one does not.
        const type = match[3] ?? value.trim();
        if (!PARTICIPANT_TYPES.has(type)) {
          refuse(`mermaid draws nothing for the type ${type}.`);
        }
        out.type = type;
      } else {
        // mermaid reads an empty alias as none, and a span has no outer space.
        if (
          value.trim() === "" ||
          (match[3] !== undefined && value !== value.trim())
        ) {
          refuse(`The alias on line ${line} is empty or has outer space.`);
        }
        if (match[3] !== undefined) {
          const at =
            content.from + start + match[0].lastIndexOf(`"${value}"`) + 1;
          out.aliasSpan = this.span(at, at + value.length);
        } else {
          const at =
            content.from + start + match[0].lastIndexOf(value.trimEnd());
          out.aliasSpan = this.span(at, at + value.trimEnd().length);
        }
        // From the body: the lexed text has its entity forms hidden.
        out.alias = this.model.body.slice(out.aliasSpan.from, out.aliasSpan.to);
      }
    }
    return out;
  }

  private create(token: SeqToken): void {
    const next = this.peek(1);
    if (next?.kind !== "participant" && next?.kind !== "participant_actor") {
      refuse(
        `Expected a participant after create on line ${this.lineText(token)}.`,
      );
    }
    this.at++;
    this.declaration(next, true, token);
  }

  private destroy(token: SeqToken): void {
    this.at++;
    const actor = this.actorToken("a participant name");
    this.endOfStatement(actor.token);
    this.addStatement(
      token,
      actor.token,
      "destroy",
      [{ name: actor.name, span: actor.span }],
      null,
    );
    this.model.destroyed.set(actor.name, this.model.records.length);
    this.waiting = { name: actor.name, create: false };
  }

  private box(token: SeqToken): void {
    this.at++;
    const rest = this.take("restOfLine", "the box's text");
    this.addStatement(
      token,
      rest,
      "boxStart",
      [],
      this.textSpan(rest.from, rest.to),
    );
    const box = { actors: [] as string[] };
    this.model.boxes.push(box);
    this.open.push({ kind: "box", box });
  }

  private end(token: SeqToken): void {
    this.at++;
    const open = this.open.pop();
    if (open === undefined) {
      refuse(`There is no block to end on line ${this.lineText(token)}.`);
    }
    const depthKind = open.kind === "box" ? "boxEnd" : "blockEnd";
    const statement = this.addStatement(token, token, depthKind, [], null);
    const block = BLOCKS[open.kind];
    if (block !== undefined) {
      this.addRecord(statement, {
        from: undefined,
        to: undefined,
        type: block.end,
        text: null,
      });
    }
  }

  private block(token: SeqToken): void {
    this.at++;
    const rest = this.take("restOfLine", "the block's text");
    const text = this.textSpan(rest.from, rest.to);
    const statement = this.addStatement(token, rest, "blockStart", [], text);
    this.addRecord(statement, {
      from: undefined,
      to: undefined,
      type: BLOCKS[token.kind]?.start ?? -1,
      text,
    });
    this.open.push({ kind: token.kind, box: null });
  }

  private middle(token: SeqToken, open: OpenFrame): void {
    this.at++;
    const rest = this.take("restOfLine", "the block's text");
    const text = this.textSpan(rest.from, rest.to);
    this.open.pop();
    const statement = this.addStatement(token, rest, "blockMiddle", [], text);
    this.open.push(open);
    this.addRecord(statement, {
      from: undefined,
      to: undefined,
      type: BLOCKS[open.kind]?.midType ?? -1,
      text,
    });
  }

  private autonumber(token: SeqToken): void {
    this.at++;
    let last = token;
    const auto: NonNullable<SeqRecord["autonumber"]> = { visible: true };
    const next = this.peek();
    if (next?.kind === "off") {
      this.at++;
      last = next;
      auto.visible = false;
    } else if (next?.kind === "NUM") {
      this.at++;
      last = next;
      auto.start = Number(this.text.slice(next.from, next.to));
      auto.step = 1;
      const second = this.peek();
      if (second?.kind === "NUM") {
        this.at++;
        last = second;
        auto.step = Number(this.text.slice(second.from, second.to));
      }
    }
    this.endOfStatement(last);
    const statement = this.addStatement(token, last, "autonumber", [], null);
    this.addRecord(statement, {
      from: undefined,
      to: undefined,
      type: AUTONUMBER,
      text: null,
      autonumber: auto,
    });
  }

  private activation(token: SeqToken): void {
    this.at++;
    const actor = this.actorToken("a participant name");
    this.endOfStatement(actor.token);
    const starts = token.kind === "activate";
    const statement = this.addStatement(
      token,
      actor.token,
      starts ? "activate" : "deactivate",
      [{ name: actor.name, span: actor.span }],
      null,
    );
    this.activated.push(actor.name);
    this.addRecord(statement, {
      from: actor.name,
      to: undefined,
      type: starts ? ACTIVE_START : ACTIVE_END,
      text: null,
    });
  }

  private note(token: SeqToken): void {
    this.at++;
    const side = this.peek();
    let placement: number;
    if (side?.kind === "left_of") {
      placement = LEFT_OF;
    } else if (side?.kind === "right_of") {
      placement = RIGHT_OF;
    } else if (side?.kind === "over") {
      placement = OVER;
    } else {
      refuse(
        `A note needs left of, right of or over, see line ${this.lineText(token)}.`,
      );
    }
    this.at++;
    const first = this.actorToken("a participant name");
    const actors = [{ name: first.name, span: first.span }];
    if (placement === OVER && this.peek()?.kind === ",") {
      this.at++;
      const second = this.actorToken("a second participant name");
      actors.push({ name: second.name, span: second.span });
    }
    const txt = this.take("TXT", "the note's text");
    this.endOfStatement(txt);
    const text = this.colonText(txt);
    const statement = this.addStatement(token, txt, "note", actors, text);
    for (const actor of actors) {
      this.addActor(actor.name, statement.line, undefined, undefined);
    }
    this.addRecord(statement, {
      from: first.name,
      to: actors[1]?.name ?? first.name,
      type: NOTE,
      text,
      placement,
    });
  }

  private link(token: SeqToken): void {
    this.at++;
    const actor = this.actorToken("a participant name");
    const txt = this.take("TXT", "the line's text");
    this.endOfStatement(txt);
    const statement = this.addStatement(
      token,
      txt,
      "link",
      [{ name: actor.name, span: actor.span }],
      this.colonText(txt),
    );
    this.addActor(actor.name, statement.line, undefined, undefined);
    if (token.kind !== "properties") {
      this.menus.push(actor.name);
    }
  }

  private acc(token: SeqToken): void {
    this.at++;
    const value = this.peek();
    if (value?.kind !== `${token.kind}_value`) {
      refuse(`Expected a value on line ${this.lineText(token)}.`);
    }
    this.at++;
    this.addStatement(token, value, "acc", [], null);
  }

  /** A block over lines is the one statement allowed to span them. It stands alone on its last line. */
  private accBlock(token: SeqToken): void {
    this.at++;
    const next = this.peek();
    if (next !== undefined && next.kind !== "NEWLINE") {
      refuse(`Text follows the accDescr block on line ${this.lineText(next)}.`);
    }
    this.addStatement(token, token, "acc", [], null, true);
  }

  private message(): void {
    const from = this.actorToken("a participant name");
    const fromToken = from.token;
    const arrowToken = this.peek();
    if (arrowToken === undefined) {
      refuse("A message has no arrow on the last line.");
    }
    if (arrowToken.kind === "()") {
      refuse(
        `The central connection on line ${this.lineText(arrowToken)} has no place in the arrow model.`,
      );
    }
    if (HALF_ARROW.test(arrowToken.kind)) {
      refuse(
        `The half arrow on line ${this.lineText(arrowToken)} has no place in the arrow model.`,
      );
    }
    const shape = ARROWS[arrowToken.kind];
    const type = ARROW_TOKEN_TYPES[arrowToken.kind];
    if (shape === undefined || type === undefined) {
      refuse(`mermaid does not read line ${this.lineText(arrowToken)}.`);
    }
    this.at++;
    let activation: SeqToken | null = null;
    const sign = this.peek();
    if (sign?.kind === "+" || sign?.kind === "-") {
      activation = sign;
      this.at++;
    } else if (sign?.kind === "()") {
      refuse(
        `The central connection on line ${this.lineText(sign)} has no place in the arrow model.`,
      );
    }
    const to = this.actorToken("the receiver's name");
    const txt = this.take("TXT", "a colon and the message's text");
    this.endOfStatement(txt);
    const text = this.colonText(txt);
    const waiting = this.waiting;
    this.waiting = null;
    if (waiting !== null) {
      const good = waiting.create
        ? to.name === waiting.name
        : to.name === waiting.name || from.name === waiting.name;
      if (!good) {
        this.refuseWaiting(waiting);
      }
    }
    const statement = this.addStatement(
      fromToken,
      txt,
      "message",
      [
        { name: from.name, span: from.span },
        { name: to.name, span: to.span },
      ],
      text,
    );
    this.addActor(from.name, statement.line, undefined, undefined);
    this.addActor(to.name, statement.line, undefined, undefined);
    const record = this.addRecord(statement, {
      from: from.name,
      to: to.name,
      type,
      text,
    });
    if (activation !== null) {
      const starts = activation.kind === "+";
      this.addRecord(statement, {
        from: starts ? to.name : from.name,
        to: undefined,
        type: starts ? ACTIVE_START : ACTIVE_END,
        text: null,
      });
      this.activated.push(starts ? to.name : from.name);
    }
    const decoded = text
      ? decodeSeqText(this.model.body.slice(text.from, text.to))
      : "";
    const key = JSON.stringify([from.name, to.name, decoded]);
    const occurrence = this.occurrences.get(key) ?? 0;
    this.occurrences.set(key, occurrence + 1);
    const message: SeqMessage = {
      line: statement.line,
      span: statement.span,
      from: from.name,
      to: to.name,
      fromSpan: from.span,
      toSpan: to.span,
      arrow: {
        span: this.span(arrowToken.from, arrowToken.to),
        ...shape,
        activation: activation && this.span(activation.from, activation.to),
      },
      text,
      record,
      occurrence,
    };
    this.model.messages.push(message);
  }
}
