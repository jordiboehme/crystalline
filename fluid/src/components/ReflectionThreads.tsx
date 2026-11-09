/**
 * The threads of one reflection, beside the page: comments anchored on a
 * passage, like familiar document comments.
 *
 * A comment is opened on what is selected on the page at the moment the
 * Comment button is pressed. Without a selection the column shows a hint in
 * the button's place; the button keeps the page's selection (its mousedown
 * is not allowed to take focus), and the anchor is computed from that
 * selection by `selectionAnchor`: a source quote with its occurrence, or the
 * section alone when the passage cannot be placed exactly.
 *
 * A thread's quote is painted on the page while it is still there, judged
 * against the text on screen rather than trusted from the answer: the page
 * and its threads arrive together, but the page is what a reader sees. Once
 * the passage is gone, the thread says so and shows the old quote as text.
 * A quote found on the page is shown without the markers whose partner it
 * cut off (`displayQuote`); one that is not found is shown as stored. What
 * is stored and sent is always the source text as it is.
 *
 * The thread a reader picked (by focus, a click in the column, or a tap on
 * its quote in the text) gets an accent wash, and its quote is painted a
 * second time on the page. From `xl` up the threads are a column beside the
 * page; below it they are a sheet behind a pill at the foot of the screen,
 * and a tap on a quote opens the sheet at its thread.
 *
 * Every call to the threads routes is made here, built from the domain and
 * the path alone, so another thread target can reuse the column later.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Clock,
  MessageSquarePlus,
  MessagesSquare,
  Reply,
  X,
} from "lucide-react";
import type { ReactElement, ReactNode, RefObject } from "react";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { Dialog } from "radix-ui";
import { useLocation } from "react-router";

import { problemDetail } from "../api/client";
import type {
  AnchorState,
  ReflectionDoc,
  SourceState,
  ThreadView,
} from "../api/reflection";
import {
  fetchThreads,
  openThread,
  reflectionKey,
  reflectionThreadsKey,
  stepThread,
} from "../api/reflection";
import type { ReflectionRight } from "../api/reflectionDomains";
import { atLeast } from "../api/reflectionDomains";
import { useAuth } from "../auth/AuthContext";
import { plural, relativeTime } from "../format";
import { REVEAL_EVENT } from "../markdown/callouts";
import { withoutHtmlComments } from "../markdown/htmlComments";
import type { RevealDetail } from "../markdown/callouts";
import type { ConventionalHead } from "../markdown/conventionalComment";
import {
  composeComment,
  formatHead,
  isBlocking,
  matchHead,
  parseConventional,
  parseHead,
} from "../markdown/conventionalComment";
import { LabelChip, LabelIcon, LabelMenu, LabelPill } from "./CommentLabel";
import { KeptComments } from "./KeptComments";
import { useSaid } from "./useSaid";
import { onlyMarkers, placeMarkers, printNotes } from "../printNotes";
import type { SelectionAnchor } from "../reflectionQuotes";
import {
  SECTION_ATTR,
  SKIP_ATTR,
  displayQuote,
  domRange,
  paintQuotes,
  selectionAnchor,
} from "../reflectionQuotes";
import { PageOutline } from "./PageOutline";
import { SelectionBar } from "./SelectionBar";
import { SELECTION_BAR_ATTR } from "./selectionBarMark";
import type { SectionText } from "../reflectionText";
import { TOP_SECTION, locateQuote } from "../reflectionText";
import {
  BUTTON,
  Chip,
  FIELD,
  FOCUS_RING,
  IconButton,
  RAIL_LABEL,
} from "./primitives";
import { refreshAfterWrite } from "../query/refreshAfterWrite";

const ALERT = "text-red-800 dark:text-red-200";

/** The quiet tier (spec 3.1): accent text, no box, for actions in a list. */
const QUIET = `rounded text-[13px] font-medium text-link hover:underline disabled:opacity-50 ${FOCUS_RING}`;

/** Tailwind's `xl`: where a 340px column fits beside the reading text. */
const COLUMN_QUERY = "(min-width: 80rem)";

function columnNow(): boolean {
  // Nothing to ask (jsdom, an old embedded view): the desktop frame, as the
  // stylesheet's own default.
  return (
    typeof window.matchMedia !== "function" ||
    window.matchMedia(COLUMN_QUERY).matches
  );
}

function subscribeColumn(onChange: () => void): () => void {
  if (typeof window.matchMedia !== "function") {
    return () => undefined;
  }
  const query = window.matchMedia(COLUMN_QUERY);
  query.addEventListener("change", onChange);
  return () => {
    query.removeEventListener("change", onChange);
  };
}

/** Whether the threads fit as a column beside the page. */
function useColumn(): boolean {
  return useSyncExternalStore(subscribeColumn, columnNow);
}

/**
 * Half-typed words and open forms, kept by the column rather than by the
 * forms: the sheet unmounts what it shows when it closes, and closing it must
 * not throw away an answer in progress.
 */
const Kept = createContext<{
  store: Map<string, unknown>;
  page: string;
} | null>(null);

/** A piece of state that outlives its component, under `key` on this page. */
function useKept<T>(key: string, initial: T): [T, (next: T) => void] {
  const kept = useContext(Kept);
  const at = `${kept?.page ?? ""}\n${key}`;
  const [value, setValue] = useState<T>(() =>
    kept?.store.has(at) ? (kept.store.get(at) as T) : initial,
  );
  const set = (next: T) => {
    if (next === initial) {
      kept?.store.delete(at);
    } else {
      kept?.store.set(at, next);
    }
    setValue(next);
  };
  return [value, set];
}

/** Forget what `useKept` holds under `key` on this page. */
function useForget(): (key: string) => void {
  const kept = useContext(Kept);
  return (key) => {
    kept?.store.delete(`${kept.page}\n${key}`);
  };
}

/**
 * A comment box's label: kept under `keep` like its words (as the head's
 * text, so no label is forgotten rather than kept), and how typed words
 * reach the box. The exact head typed or pasted at the very start of the box
 * becomes the chip while the box has none; once a person takes a chip off,
 * what they type stays text. That choice is kept beside the label (under
 * `${keep}-removed`), so a sheet shut and opened again never turns their
 * plain words into a chip.
 */
function useLabel(
  keep: string,
  setWords: (words: string) => void,
): {
  head: ConventionalHead | null;
  setHead: (head: ConventionalHead | null) => void;
  /** Takes the chip off; a head typed after it stays text. */
  remove: () => void;
  /** The box's words changed to `value`. */
  type: (value: string) => void;
} {
  const [raw, setRaw] = useKept(keep, "");
  const [removed, setRemoved] = useKept(`${keep}-removed`, false);
  const head = raw === "" ? null : parseHead(raw);
  const setHead = (next: ConventionalHead | null) => {
    setRaw(next === null ? "" : formatHead(next));
  };
  return {
    head,
    setHead,
    remove: () => {
      setRemoved(true);
      setHead(null);
    },
    type: (value) => {
      const match = head === null && !removed ? matchHead(value) : null;
      if (match !== null) {
        setHead(match.head);
        setWords(value.slice(match.end));
      } else {
        setWords(value);
      }
    },
  };
}

/** The label of a thread's opening comment, or null for a plain one. */
function openingHead(thread: ThreadView): ConventionalHead | null {
  const first = thread.entries[0];
  return first?.kind === "comment" ? parseConventional(first.text) : null;
}

/**
 * An entry's words: a labelled comment as its pill and its subject in bold,
 * then the discussion; anything else as written.
 */
function EntryWords({ entry }: { entry: ThreadView["entries"][number] }) {
  const words =
    "col-start-2 text-[13.5px] leading-[1.55] whitespace-pre-wrap text-ink-2";
  const labelled =
    entry.kind === "comment" ? parseConventional(entry.text) : null;
  if (labelled === null) {
    return (
      <p className={words}>
        {entry.kind === "resolve"
          ? `Resolved: ${entry.text}`
          : entry.kind === "reopen"
            ? entry.text === ""
              ? "Reopened"
              : `Reopened: ${entry.text}`
            : entry.text}
      </p>
    );
  }
  return (
    <div className="col-start-2 flex flex-col gap-1">
      <p className={words}>
        <LabelPill head={labelled} />
        <strong className="font-semibold text-ink">{labelled.subject}</strong>
      </p>
      {labelled.discussion.trim() !== "" && (
        <p className={words}>{labelled.discussion}</p>
      )}
    </div>
  );
}

/**
 * A quiet switch with its words beside it, as the column's head draws its
 * filters.
 */
function Switch({
  on,
  label,
  words,
  onChange,
}: {
  on: boolean;
  /** The accessible name. */
  label: string;
  /** What is shown. */
  words: string;
  onChange: (on: boolean) => void;
}): ReactElement {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={() => {
        onChange(!on);
      }}
      className={`inline-flex items-center gap-2 rounded text-[12.5px] whitespace-nowrap text-muted hover:text-ink-3 ${FOCUS_RING}`}
    >
      <span
        aria-hidden
        className={`relative h-[15px] w-[26px] shrink-0 rounded-full transition-colors motion-reduce:transition-none ${
          on ? "bg-accent-600 dark:bg-accent-400" : "bg-slate-500 dark:bg-faint"
        }`}
      >
        <span
          className={`absolute top-0.5 left-0.5 size-[11px] rounded-full bg-white shadow-sm transition-transform motion-reduce:transition-none ${
            on ? "translate-x-[11px]" : ""
          }`}
        />
      </span>
      {words}
    </button>
  );
}

/**
 * The text nodes of `range` outside notes the page drew (`data-anchor-skip`):
 * a quote that runs over a heading paints the heading's words, not the
 * sources control or the title note beside them.
 */
function paintable(range: Range): Range[] {
  const root = range.commonAncestorContainer;
  const host =
    root.nodeType === Node.TEXT_NODE ? root.parentElement : (root as Element);
  if (host === null || host.querySelector(`[${SKIP_ATTR}]`) === null) {
    return [range];
  }
  const parts: Range[] = [];
  const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (
      !range.intersectsNode(node) ||
      node.parentElement?.closest(`[${SKIP_ATTR}]`)
    ) {
      continue;
    }
    const part = document.createRange();
    part.selectNodeContents(node);
    if (node === range.startContainer) {
      part.setStart(node, range.startOffset);
    }
    if (node === range.endContainer) {
      part.setEnd(node, range.endOffset);
    }
    if (!part.collapsed) {
      parts.push(part);
    }
  }
  return parts;
}

/** A caret position under a point, in the browsers that answer it. */
function caretAt(x: number, y: number): { node: Node; offset: number } | null {
  const doc = document as Document & {
    caretPositionFromPoint?: (
      x: number,
      y: number,
    ) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  if (typeof doc.caretPositionFromPoint === "function") {
    const at = doc.caretPositionFromPoint(x, y);
    return at === null ? null : { node: at.offsetNode, offset: at.offset };
  }
  if (typeof doc.caretRangeFromPoint === "function") {
    const at = doc.caretRangeFromPoint(x, y);
    return at === null
      ? null
      : { node: at.startContainer, offset: at.startOffset };
  }
  return null;
}

/** A quote, with an empty one read as none, as the server reads it. */
function quoteOf(quote: string | null): string | null {
  return quote === "" ? null : quote;
}

/**
 * The section a thread is anchored on: by id, or by heading for a section
 * with no id yet. Two id-less sections with one heading are rare (the engine
 * gives every section an id on its next write); the first one is taken, as
 * the engine's `find_section` does.
 */
function sectionOf(
  key: string,
  sections: readonly SectionText[],
): SectionText | undefined {
  return sections.find((s) => (s.id ?? s.heading) === key);
}

/** Where a thread stands on the text on screen. */
// eslint-disable-next-line react-refresh/only-export-components
export function effectiveAnchor(
  thread: ThreadView,
  sections: readonly SectionText[],
): AnchorState {
  const section = sectionOf(thread.anchor.section, sections);
  if (section === undefined) {
    return "section_gone";
  }
  const quote = quoteOf(thread.anchor.quote);
  if (quote === null) {
    return "found";
  }
  return locateQuote(section.text, quote, thread.anchor.occurrence) === null
    ? "text_changed"
    : "found";
}

/** The heading a thread is shown under, or null for the intro. */
function headingOf(
  thread: ThreadView,
  sections: readonly SectionText[],
): string | null {
  const section = sectionOf(thread.anchor.section, sections);
  // A comment in the heading is a note for whoever edits it.
  return section === undefined || section.id === TOP_SECTION
    ? null
    : withoutHtmlComments(section.heading).trim();
}

/**
 * A quote as a person is shown it: cleaned of cut markup where it is found
 * in `source` (the text of its section), else exactly as stored.
 */
function shownQuote(
  quote: string,
  occurrence: number,
  source: string | null,
): string {
  const at = source === null ? null : locateQuote(source, quote, occurrence);
  if (source === null || at === null) {
    // As stored, less a comment the page would not draw.
    const shown = withoutHtmlComments(quote);
    return shown.trim() === "" ? quote : shown;
  }
  return displayQuote(source, at.start, at.end);
}

/** Colours a person's initial is drawn on; white text reads on each. */
const AVATAR_COLORS = [
  "#6c5eb5",
  "#0f766e",
  "#b45309",
  "#be185d",
  "#1d4ed8",
  "#4d7c0f",
] as const;

/** The same colour for the same name, on every page. */
function avatarColor(name: string): string {
  let hash = 0;
  for (const char of name) {
    hash = (hash * 31 + (char.codePointAt(0) ?? 0)) >>> 0;
  }
  return AVATAR_COLORS[hash % AVATAR_COLORS.length] ?? AVATAR_COLORS[0];
}

/** A round initial; the name beside it is what a screen reader hears. */
export function Avatar({
  name,
  agent,
}: {
  name: string;
  agent: boolean;
}): ReactElement {
  const initial = (Array.from(name.trim())[0] ?? "?").toUpperCase();
  return agent ? (
    <span
      aria-hidden
      data-avatar=""
      className="mt-px inline-grid size-[22px] place-items-center rounded-full border border-hair-strong bg-sunken text-[11px] font-semibold text-muted"
    >
      {initial}
    </span>
  ) : (
    <span
      aria-hidden
      data-avatar=""
      style={{ backgroundColor: avatarColor(name) }}
      className="mt-px inline-grid size-[22px] place-items-center rounded-full text-[11px] font-semibold text-white"
    >
      {initial}
    </span>
  );
}

/**
 * The white space Rust's `split_whitespace` splits on (Unicode White_Space):
 * unlike JavaScript's `\s` it splits on U+0085 and not on a byte order mark.
 */
const CORE_SPACE =
  /[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+/;

/**
 * An account name as a thread entry stores it (core `sanitize_name`):
 * parentheses, colons, commas and line breaks become spaces, runs of spaces
 * collapse, and an empty name is `someone`.
 */
function entryName(name: string): string {
  const joined = name
    .replace(/[():,\n\r]/g, " ")
    .split(CORE_SPACE)
    .filter((word) => word !== "")
    .join(" ");
  return joined === "" ? "someone" : joined;
}

/**
 * Who an open thread waits on, in words. A thread a person opened and an
 * agent answered last waits for that person, who resolves it: the cue says
 * so, to them as "you".
 */
// eslint-disable-next-line react-refresh/only-export-components
export function waitingWords(
  thread: ThreadView,
  viewer: string | null,
): { words: string; answered: boolean } {
  if (thread.waitingOn === "agent") {
    return { words: "Waiting for the agent", answered: false };
  }
  const opener = thread.entries[0];
  const last = thread.entries.at(-1);
  if (
    opener === undefined ||
    opener.author.agent ||
    last === undefined ||
    !last.author.agent ||
    last.kind !== "comment"
  ) {
    return { words: "Waiting for people", answered: false };
  }
  const you = viewer !== null && entryName(viewer) === opener.author.name;
  return {
    words: `Answered, waiting for ${you ? "you" : opener.author.name}`,
    answered: true,
  };
}

export function ThreadCard({
  thread,
  heading,
  state,
  actions,
  source = null,
  picked = false,
  viewer = null,
}: {
  thread: ThreadView;
  heading: string | null;
  state: AnchorState;
  actions: ReactNode;
  /** The text of the thread's section; without it the quote shows as stored. */
  source?: string | null;
  /** The thread the reader picked: its quote rule turns accent. */
  picked?: boolean;
  /** The signed-in account's name, so a thread waiting on them says "you". */
  viewer?: string | null;
}): ReactElement {
  const now = new Date();
  const quote = quoteOf(thread.anchor.quote);
  const opening = openingHead(thread);
  const cue = waitingWords(thread, viewer);
  const Icon = cue.answered ? Reply : Clock;
  const waiting =
    thread.status === "open" ? (
      <span
        className={`ml-auto inline-flex items-center gap-1.5 text-xs ${
          cue.answered ? "text-ink-3" : "text-muted"
        }`}
      >
        <Icon aria-hidden size={14} />
        {cue.words}
      </span>
    ) : null;
  return (
    <article
      id={thread.id}
      // Focusable by script only: a tap on its quote in the text, or the
      // sheet opening at it, puts the keyboard here.
      tabIndex={-1}
      aria-label={`Thread on ${heading ?? "the page"}`}
      className="flex flex-col gap-3 text-sm outline-none"
    >
      {quote !== null && (
        <blockquote
          className={`flex gap-1.5 border-l-2 py-px pl-2.5 text-[13px] leading-normal text-ink-3 ${
            picked ? "border-accent-400" : "border-(--mark-rule)"
          }`}
        >
          {/* The opening comment's label, beside the passage it is about. */}
          {opening !== null && (
            <span className="mt-[3px]">
              <LabelIcon head={opening} size={13} alone />
            </span>
          )}
          <span className="min-w-0">
            {shownQuote(quote, thread.anchor.occurrence, source)}
          </span>
        </blockquote>
      )}
      {state === "text_changed" && (
        <p className="-mt-1 text-[12.5px] text-muted">The text changed.</p>
      )}
      <ol className="flex flex-col gap-2.5">
        {thread.entries.map((entry, index) => (
          <li
            key={`${entry.at}-${String(index)}`}
            className="grid grid-cols-[22px_minmax(0,1fr)] gap-x-2.5 gap-y-0.5"
          >
            <Avatar name={entry.author.name} agent={entry.author.agent} />
            <p className="flex flex-wrap items-baseline gap-x-2 text-[13px]">
              <span className="font-semibold text-ink">
                {entry.author.name}
                {entry.author.agent && (
                  <span className="font-normal text-muted"> (agent)</span>
                )}
              </span>
              <span className="text-xs text-muted">
                {relativeTime(entry.at, now) ?? ""}
              </span>
            </p>
            <EntryWords entry={entry} />
          </li>
        ))}
      </ol>
      {(actions !== null || waiting !== null) && (
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2 pl-8">
          {actions}
          {waiting}
        </div>
      )}
    </article>
  );
}

/** A source's state as a chip; a current source needs none. */
// eslint-disable-next-line react-refresh/only-export-components
export function sourceChip(state: SourceState): ReactElement | null {
  switch (state) {
    case "current":
      return null;
    case "changed":
      return <Chip variant="caution">changed</Chip>;
    case "retired":
      return <Chip variant="retired">retired</Chip>;
    case "missing":
      return <Chip variant="danger">missing</Chip>;
    case "unavailable":
      return <Chip variant="retired">not checked here</Chip>;
  }
}

interface Listed {
  thread: ThreadView;
  state: AnchorState;
  heading: string | null;
  source: string | null;
}

export function ReflectionThreads({
  domain,
  path,
  doc,
  sections,
  right,
  articleRef,
}: {
  domain: string;
  path: string;
  doc: ReflectionDoc;
  sections: SectionText[];
  right: ReflectionRight;
  articleRef: RefObject<HTMLElement | null>;
}): ReactElement {
  const queryClient = useQueryClient();
  const { user, capabilities } = useAuth();
  const { hash } = useLocation();
  const refresh = () =>
    refreshAfterWrite(queryClient, { queryKey: reflectionKey(domain) });
  // Reading and commenting are one level; the anonymous viewer of a public
  // domain is the one reader who never comments (spec B5). A server that
  // serves read-only refuses every thread write, so it gets no controls.
  const mayComment =
    atLeast(right, "read") && user !== null && !capabilities.readOnly;
  const column = useColumn();
  const [captured, setCaptured] = useState<SelectionAnchor | null>(null);
  const [drafting, setDrafting] = useState<SelectionAnchor | null>(null);
  // The thread under the pointer, and the one the reader picked. The pointer
  // wins for the second paint on the page; only the pick gets the wash.
  const [hovered, setHovered] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [showResolved, setShowResolved] = useState(false);
  const [blockingOnly, setBlockingOnly] = useState(false);
  const [sheetOpen, setSheetOpen] = useState(false);
  // The page's selection while it is live in the page (not collapsed, not
  // gone elsewhere), as a range of its own: the bar at the selection stands
  // by it and measures it again when the text moves. Unlike `captured`, a
  // caret outside the page takes it away.
  const [selectedAt, setSelectedAt] = useState<Range | null>(null);
  // A press on the bar in progress, with the passage it stood by: a touch
  // can collapse the selection in the page before the click lands, and the
  // click must still comment on that passage.
  const [barPress, setBarPress] = useState<{
    anchor: SelectionAnchor;
    range: Range;
  } | null>(null);
  // Whether the pointer button went down on the page and is still down: a
  // drag selection, which the bar must not stand in the way of.
  const [pointerDown, setPointerDown] = useState(false);
  // The passage Escape sent the bar away from, by value: the key coming up
  // reads the same selection again, as a new object.
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [store] = useState(() => new Map<string, unknown>());
  const kept = useMemo(
    () => ({ store, page: `${domain}/${path}` }),
    [store, domain, path],
  );
  const commentKey = `${kept.page}\n${COMMENT_KEY}`;
  const commentLabelKey = `${kept.page}\n${COMMENT_LABEL_KEY}`;
  const [said, say] = useSaid(4000);
  // Whether the bar was announced for the passage captured now, so it is
  // announced once per selection, not at every change of it.
  const announced = useRef(false);
  // The thread a comment just opened, focused once the page lists it.
  const focusNew = useRef<string | null>(null);
  const commentButton = useRef<HTMLButtonElement | null>(null);
  const pillRef = useRef<HTMLButtonElement | null>(null);
  const threadsHeading = useRef<HTMLHeadingElement | null>(null);
  // Set when the form closes, so focus goes back to the button once it is
  // drawn again (or to the heading when the selection went with the send).
  const backToComment = useRef(false);
  // The painted quotes by thread, for a tap on one in the text.
  const painted = useRef(new Map<string, Range>());
  // The threads the column lists now, for a tap on a quote whose card a
  // filter hides.
  const listedIds = useRef(new Set<string>());
  // Whether the page is printing, while the numbers are in its text.
  const printing = useRef(false);
  // The thread the sheet opens at, once, when a tap or a link asked for it.
  const [openAt, setOpenAt] = useState<string | null>(null);
  // A link to a thread (`#t-<hex>`) picks it; on a narrow screen it also
  // opens the sheet at it, as the column that would hold it is not there.
  const [seenHash, setSeenHash] = useState<string | null>(null);
  if (hash !== seenHash) {
    setSeenHash(hash);
    const id = decodeURIComponent(hash.slice(1));
    if (doc.threads.some((t) => t.id === id)) {
      setPicked(id);
      if (!column) {
        setOpenAt(id);
        setSheetOpen(true);
      }
    }
  }
  // Crossing the breakpoint shuts the sheet, as the sidebar drawer does: the
  // column that takes over shows the same threads.
  const [seenColumn, setSeenColumn] = useState(column);
  if (column !== seenColumn) {
    setSeenColumn(column);
    setSheetOpen(false);
  }

  useEffect(() => {
    if (drafting === null && backToComment.current) {
      backToComment.current = false;
      (commentButton.current ?? threadsHeading.current)?.focus();
    }
  }, [drafting]);

  // The page's selection as it stands, or null when there is none in the
  // page (Tab to the button collapses it). The Comment button reads it again
  // when pressed, in case the last change of it was not heard.
  const selectionNow = (): SelectionAnchor | null => {
    const root = articleRef.current;
    const selection = window.getSelection();
    if (root === null || selection === null || selection.rangeCount === 0) {
      return null;
    }
    const range = selection.getRangeAt(0);
    return range.collapsed || !root.contains(range.commonAncestorContainer)
      ? null
      : selectionAnchor(root, range, sections);
  };

  // What is selected on the page. A collapse on the page clears it, and so
  // does text selected elsewhere; a caret or a field outside the page
  // changes nothing.
  useEffect(() => {
    const read = () => {
      const root = articleRef.current;
      const selection = window.getSelection();
      if (root === null || selection === null || selection.rangeCount === 0) {
        return;
      }
      // Alt+F10 put the focus in the bar at the selection: whatever the
      // browser does to the selection then, the passage stays the one the
      // bar stands by.
      if (document.activeElement?.closest(`[${SELECTION_BAR_ATTR}]`)) {
        return;
      }
      const range = selection.getRangeAt(0);
      if (!root.contains(range.commonAncestorContainer)) {
        setSelectedAt(null);
        // Text selected elsewhere (a thread card) replaces the page's
        // passage; a caret or typing in a field leaves it alone.
        const typing =
          document.activeElement instanceof HTMLInputElement ||
          document.activeElement instanceof HTMLTextAreaElement;
        if (!range.collapsed && !typing) {
          setCaptured(null);
        }
        return;
      }
      const next = range.collapsed
        ? null
        : selectionAnchor(root, range, sections);
      setCaptured(next);
      setSelectedAt(next === null ? null : range.cloneRange());
    };
    // A keyboard or pointer selection is read again when the key or the
    // button comes up: Chrome can leave the last steps of a fast Shift+Arrow
    // run without a selectionchange the page hears before focus moves on
    // (Tab collapses the selection), and the quote would come out short.
    document.addEventListener("selectionchange", read);
    document.addEventListener("keyup", read);
    document.addEventListener("pointerup", read);
    return () => {
      document.removeEventListener("selectionchange", read);
      document.removeEventListener("keyup", read);
      document.removeEventListener("pointerup", read);
    };
  }, [articleRef, sections]);

  // The pointer button on the page: while it is down the reader is
  // selecting, and the bar waits. A press that comes up ends an Escape held
  // for the last passage; a touch scroll the browser takes over ends in a
  // cancel, which picked nothing.
  useEffect(() => {
    const root = articleRef.current;
    if (root === null) {
      return;
    }
    let pressed = false;
    const onDown = (event: PointerEvent) => {
      if (event.isPrimary === false || event.button !== 0) {
        return;
      }
      pressed = true;
      setPointerDown(true);
    };
    const onUp = (event: PointerEvent) => {
      if (!pressed || event.isPrimary === false) {
        return;
      }
      pressed = false;
      setPointerDown(false);
      setDismissed(null);
    };
    const onCancel = (event: PointerEvent) => {
      if (!pressed || event.isPrimary === false) {
        return;
      }
      pressed = false;
      setPointerDown(false);
    };
    root.addEventListener("pointerdown", onDown);
    document.addEventListener("pointerup", onUp);
    document.addEventListener("pointercancel", onCancel);
    return () => {
      root.removeEventListener("pointerdown", onDown);
      document.removeEventListener("pointerup", onUp);
      document.removeEventListener("pointercancel", onCancel);
    };
  }, [articleRef]);

  // An Escape holds only for the passage it was pressed on. Set during
  // render, React's pattern for state that follows other state.
  const capturedKey = captured === null ? null : passageKey(captured);
  if (dismissed !== null && dismissed !== capturedKey) {
    setDismissed(null);
  }

  const resolved = useQuery({
    queryKey: reflectionThreadsKey(domain, path, "resolved"),
    queryFn: () => fetchThreads(domain, { path, status: "resolved" }),
    enabled: showResolved,
  });

  const list = (thread: ThreadView): Listed => ({
    thread,
    state: effectiveAnchor(thread, sections),
    heading: headingOf(thread, sections),
    source: sectionOf(thread.anchor.section, sections)?.text ?? null,
  });

  const open = useMemo(
    () =>
      doc.threads
        .filter((t) => t.status === "open")
        .map((thread) => ({
          thread,
          state: effectiveAnchor(thread, sections),
        })),
    [doc.threads, sections],
  );

  const shown = hovered ?? picked;
  // The thread whose quote the page was last asked to show, so a repaint
  // does not open again a fold the reader closed.
  const pickedNow = useRef(picked);
  const revealed = useRef<string | null>(null);
  useEffect(() => {
    pickedNow.current = picked;
    if (picked === null) {
      revealed.current = null;
    }
  });
  // A picked thread's quote may sit in a folded callout: ask the page to
  // show it (`REVEAL_EVENT`, which `MarkdownBody` answers by opening the
  // fold). Once per pick, as soon as the quote is painted.
  const revealPicked = useCallback(() => {
    const id = pickedNow.current;
    const node =
      id === null ? undefined : painted.current.get(id)?.startContainer;
    if (id === null || node === undefined || revealed.current === id) {
      return;
    }
    const target =
      node.nodeType === Node.TEXT_NODE ? node.parentElement : (node as Element);
    if (target === null) {
      return;
    }
    revealed.current = id;
    target.dispatchEvent(
      new CustomEvent<RevealDetail>(REVEAL_EVENT, {
        bubbles: true,
        detail: { target },
      }),
    );
  }, []);

  // Words kept for a thread that is no longer open (resolved, or gone) have
  // nowhere to go back to.
  useEffect(() => {
    const live = new Set(open.map((o) => o.thread.id));
    const prefix = `${kept.page}\n`;
    for (const key of [...kept.store.keys()]) {
      if (!key.startsWith(prefix)) {
        continue;
      }
      const id = key.slice(prefix.length).split(":")[0] ?? "";
      if (id.startsWith("t-") && !live.has(id)) {
        kept.store.delete(key);
      }
    }
  }, [open, kept]);

  // A comment just sent: its thread takes the keyboard once it is listed,
  // read out by its name.
  useEffect(() => {
    const id = focusNew.current;
    if (id === null || !doc.threads.some((t) => t.id === id)) {
      return;
    }
    focusNew.current = null;
    document.getElementById(id)?.focus();
  }, [doc.threads]);

  // Paint the quotes still on the page, again whenever the rendered page
  // changes: the sections render lazily, so the first paint may find nothing.
  useEffect(() => {
    const root = articleRef.current;
    if (root === null) {
      return;
    }
    let stop: () => void = () => undefined;
    const paint = () => {
      stop();
      const all: Range[] = [];
      const current: Range[] = [];
      painted.current.clear();
      for (const { thread, state } of open) {
        const quote = quoteOf(thread.anchor.quote);
        if (state !== "found" || quote === null) {
          continue;
        }
        const section = sectionOf(thread.anchor.section, sections);
        // Compared by attribute rather than by selector, so a heading used
        // as a key (a section with no id yet) needs no escaping.
        const scope =
          Array.from(root.querySelectorAll(`[${SECTION_ATTR}]`)).find(
            (el) => el.getAttribute(SECTION_ATTR) === thread.anchor.section,
          ) ?? null;
        const at = section
          ? locateQuote(section.text, quote, thread.anchor.occurrence)
          : null;
        const range =
          scope && section && at
            ? domRange(scope, at.start, at.end, section.text)
            : null;
        if (range !== null) {
          const parts = paintable(range);
          all.push(...parts);
          painted.current.set(thread.id, range);
          if (thread.id === shown) {
            current.push(...parts);
          }
        }
      }
      stop = paintQuotes(all, current);
      revealPicked();
    };
    paint();
    // While the page prints, the numbers put in the text are no change to
    // it: the quotes stay painted as they were.
    const observer = new MutationObserver((records) => {
      if (!printing.current && !onlyMarkers(records)) {
        paint();
      }
    });
    observer.observe(root, { childList: true, subtree: true });
    return () => {
      observer.disconnect();
      stop();
    };
  }, [articleRef, open, sections, shown, revealPicked]);
  // A pick while another thread is under the pointer paints nothing new.
  useEffect(() => {
    revealPicked();
  }, [picked, revealPicked]);

  // On paper each open thread is a numbered note at the end of the page
  // (`PrintNotes`), and its number stands at the end of the block that holds
  // the end of its quote. The numbers go in as printing starts and come out
  // as it ends: on screen the text never holds them. A change to the threads
  // while the print dialog is open leaves the numbers in until it closes.
  const notes = useMemo(
    () => printNotes(doc.threads, sections),
    [doc.threads, sections],
  );
  const notesRef = useRef(notes);
  useEffect(() => {
    notesRef.current = notes;
  });
  useEffect(() => {
    let undo: (() => void) | null = null;
    const start = () => {
      if (undo !== null) {
        return;
      }
      printing.current = true;
      const numbers = new Map(
        notesRef.current.map((note) => [note.thread.id, note.n]),
      );
      const marks: { range: Range; n: number }[] = [];
      for (const [id, range] of painted.current) {
        const n = numbers.get(id);
        if (n !== undefined) {
          marks.push({ range, n });
        }
      }
      undo = placeMarkers(marks);
    };
    const end = () => {
      undo?.();
      undo = null;
      printing.current = false;
    };
    const media =
      typeof window.matchMedia === "function"
        ? window.matchMedia("print")
        : null;
    const onChange = (event: MediaQueryListEvent) => {
      if (event.matches) {
        start();
      } else {
        end();
      }
    };
    window.addEventListener("beforeprint", start);
    window.addEventListener("afterprint", end);
    media?.addEventListener("change", onChange);
    return () => {
      window.removeEventListener("beforeprint", start);
      window.removeEventListener("afterprint", end);
      media?.removeEventListener("change", onChange);
      end();
    };
  }, []);

  // A tap on a painted quote picks its thread; below `xl` it opens the
  // sheet at it. A tap anywhere else in the text lets the pick go. A drag
  // that selected text is a selection, not a tap.
  useEffect(() => {
    const root = articleRef.current;
    if (root === null) {
      return;
    }
    const onClick = (event: MouseEvent) => {
      // A control on the page (the sources count, a link symbol) or a note
      // it drew is no tap on the text.
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest(`[${SKIP_ATTR}], button`)) {
        return;
      }
      const selection = window.getSelection();
      if (selection !== null && !selection.isCollapsed) {
        return;
      }
      const at = caretAt(event.clientX, event.clientY);
      const hit =
        at === null
          ? undefined
          : Array.from(painted.current).find(([, range]) => {
              try {
                return range.isPointInRange(at.node, at.offset);
              } catch {
                return false;
              }
            });
      if (hit === undefined) {
        setPicked(null);
        return;
      }
      // Blocking only lists the blocking threads, and every quote stays
      // painted: a tap on another one shows all threads again, so its card
      // is there to be picked.
      if (!listedIds.current.has(hit[0])) {
        setBlockingOnly(false);
      }
      setPicked(hit[0]);
      if (!column) {
        setOpenAt(hit[0]);
        setSheetOpen(true);
      }
    };
    root.addEventListener("click", onClick);
    return () => {
      root.removeEventListener("click", onClick);
    };
  }, [articleRef, column]);

  const comment = useMutation({
    mutationFn: ({ anchor, text }: { anchor: SelectionAnchor; text: string }) =>
      openThread(domain, {
        path,
        section: anchor.section,
        ...(quoteOf(anchor.quote) !== null
          ? { quote: anchor.quote ?? "", occurrence: anchor.occurrence ?? 1 }
          : {}),
        text,
      }),
    onSuccess: (thread) => {
      kept.store.delete(commentKey);
      kept.store.delete(commentLabelKey);
      kept.store.delete(`${commentLabelKey}-removed`);
      focusNew.current = thread?.id ?? null;
      // The new thread must be listed to take the keyboard.
      setBlockingOnly(false);
      say("Comment posted.");
      backToComment.current = true;
      setDrafting(null);
      setCaptured(null);
      window.getSelection()?.removeAllRanges();
      void refresh();
    },
  });

  // The column's Comment button and the bar at the selection: a comment on
  // what is selected now, or on what was when the selection was last heard.
  const startComment = (pressed: SelectionAnchor | null = null) => {
    comment.reset();
    setDrafting(selectionNow() ?? captured ?? pressed);
  };

  // The bar stands by a live selection in the page, for whoever may
  // comment, while no comment is written, the pointer is not selecting,
  // Escape did not send it away and the sheet does not cover it. A press
  // on it keeps it there until the press ends, whatever the selection does.
  const barAt =
    !mayComment || drafting !== null || sheetOpen
      ? null
      : barPress !== null
        ? barPress.range
        : captured !== null && !pointerDown && dismissed === null
          ? selectedAt
          : null;
  const bar =
    barAt === null ? null : (
      <SelectionBar
        range={barAt}
        frame={articleRef}
        onPressStart={() => {
          // Always the passage of this press, never one an earlier press kept.
          setBarPress(
            captured !== null && selectedAt !== null
              ? { anchor: captured, range: selectedAt }
              : null,
          );
        }}
        onPressEnd={() => {
          setBarPress(null);
        }}
        onLost={() => {
          setSelectedAt(null);
          setBarPress(null);
        }}
        onComment={() => {
          startComment(barPress?.anchor ?? null);
          setBarPress(null);
          // Below `xl` the form is in the sheet.
          if (!column) {
            setSheetOpen(true);
          }
        }}
        onDismiss={() => {
          setDismissed(capturedKey);
        }}
      />
    );
  const barShown = barAt !== null;
  // Said once per selection, and only while the bar is there to be used:
  // not to a reader who may not comment, not while a comment is written.
  useEffect(() => {
    if (captured === null) {
      announced.current = false;
      return;
    }
    if (barShown && !announced.current) {
      announced.current = true;
      // Below `xl` the column's button is in the sheet.
      say(
        column
          ? "Comment on the selection with Alt+F10"
          : "Comment on the selection with Alt+F10 or from the threads",
      );
    }
  }, [barShown, captured, column, say]);

  // A thread a resolve or reopen moves is drawn elsewhere: focus waits on
  // the column's heading rather than on a control that is gone.
  const afterMove = () => {
    threadsHeading.current?.focus();
    return refresh();
  };

  // The open threads whose opening comment blocks. The filter holds only
  // while there is one: once the last is resolved, its switch goes and the
  // list shows everything again rather than nothing.
  const blocking = open.filter((o) => {
    const head = openingHead(o.thread);
    return head !== null && isBlocking(head);
  });
  const onlyBlocking = blockingOnly && blocking.length > 0;
  // Open threads, then the resolved ones when asked for; a thread both
  // lists hold for a moment (between a step and the refetch) shows once.
  const openIds = new Set(open.map((o) => o.thread.id));
  const listed: Listed[] = onlyBlocking
    ? blocking.map((o) => list(o.thread))
    : [
        ...open.map((o) => list(o.thread)),
        ...(showResolved ? (resolved.data ?? []) : [])
          .filter((t) => t.status === "resolved" && !openIds.has(t.id))
          .map(list),
      ];
  useEffect(() => {
    listedIds.current = new Set(listed.map((item) => item.thread.id));
  });
  const bySection = new Map<string, Listed[]>();
  for (const item of listed) {
    const key = item.state === "section_gone" ? "" : item.thread.anchor.section;
    bySection.set(key, [...(bySection.get(key) ?? []), item]);
  }
  // Two id-less sections with one heading share a key; it is listed once.
  const order = [...new Set([...sections.map((s) => s.id ?? s.heading), ""])];

  const heading = (
    <h2
      ref={threadsHeading}
      tabIndex={-1}
      className="text-[15px] font-semibold text-ink outline-none"
    >
      Threads
    </h2>
  );

  // The page's `##` sections, linked to the section elements the page draws
  // (`s-<id>`, see `PageSection`): this page has no heading ids of its own.
  const outline = sections.flatMap((s, index) =>
    s.id === TOP_SECTION || s.heading === ""
      ? []
      : [
          {
            id: s.id === null ? `s-h${String(index)}` : `s-${s.id}`,
            text: withoutHtmlComments(s.heading).trim(),
            level: 2 as const,
          },
        ],
  );

  const body = (
    <>
      <PageOutline
        headings={outline}
        onNavigate={
          column
            ? undefined
            : () => {
                setSheetOpen(false);
              }
        }
      />
      <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1.5">
        {column ? heading : <Dialog.Title asChild>{heading}</Dialog.Title>}
        {open.length > 0 && (
          <span className="text-xs text-muted">{`${String(open.length)} open`}</span>
        )}
        <span className="ml-auto flex flex-wrap items-center justify-end gap-x-3 gap-y-1.5">
          {blocking.length > 0 && (
            <Switch
              on={onlyBlocking}
              label="Show blocking threads only"
              words="Blocking only"
              onChange={setBlockingOnly}
            />
          )}
          <Switch
            on={showResolved}
            label="Show resolved threads"
            words="Show resolved"
            onChange={setShowResolved}
          />
        </span>
        {!column && (
          <IconButton
            label="Close the threads"
            icon={X}
            onClick={() => {
              setSheetOpen(false);
            }}
          />
        )}
      </div>
      <KeptComments domain={domain} path={path} />
      {mayComment &&
        drafting === null &&
        (captured === null ? (
          <p className="flex items-center gap-2.5 rounded-[10px] border border-dashed border-hair-strong px-3 py-2.5 text-[13px] text-muted">
            <MessageSquarePlus aria-hidden size={16} className="shrink-0" />
            Select text in the page to comment on it.
          </p>
        ) : (
          <button
            ref={commentButton}
            type="button"
            // Keep the page's selection: a press that took focus would
            // collapse it before the click is handled.
            onMouseDown={(event) => {
              event.preventDefault();
            }}
            onClick={() => {
              startComment();
            }}
            className={`flex items-center gap-2.5 rounded-[10px] border border-accent-300 bg-accent-50 px-3 py-2.5 text-left text-[13px] font-medium text-accent-800 hover:bg-accent-100 dark:border-accent-700 dark:bg-accent-400/10 dark:text-accent-200 dark:hover:bg-accent-400/20 ${FOCUS_RING}`}
          >
            <MessageSquarePlus aria-hidden size={16} className="shrink-0" />
            Comment
          </button>
        ))}
      {showResolved && resolved.error && (
        <p role="alert" className={`text-sm ${ALERT}`}>
          {problemDetail(resolved.error)}
        </p>
      )}
      {drafting !== null && (
        <CommentForm
          anchor={drafting}
          source={sectionOf(drafting.section, sections)?.text ?? null}
          pending={comment.isPending}
          error={comment.error}
          onCancel={() => {
            kept.store.delete(commentKey);
            kept.store.delete(commentLabelKey);
            kept.store.delete(`${commentLabelKey}-removed`);
            comment.reset();
            backToComment.current = true;
            setDrafting(null);
          }}
          onSubmit={(text) => {
            comment.mutate({ anchor: drafting, text });
          }}
        />
      )}
      <div className="flex flex-col">
        {order.map((key) => {
          const items = bySection.get(key);
          if (!items) {
            return null;
          }
          return (
            <div key={key || "gone"} className="flex flex-col">
              {key === "" && (
                <h3 className={`border-t border-hair pt-4 ${RAIL_LABEL}`}>
                  Not on the page any more
                </h3>
              )}
              {items.map(({ thread, state, heading: on, source }) => (
                <div
                  key={thread.id}
                  data-picked={picked === thread.id ? "" : undefined}
                  className={
                    picked === thread.id
                      ? // Muted text turns ink-3 on the wash, to keep 4.5:1.
                        "-mx-3 rounded-[10px] bg-accent-50 px-3 py-4 dark:bg-accent-400/16 [&_.text-muted]:text-ink-3"
                      : "border-t border-hair py-4"
                  }
                  onFocusCapture={() => {
                    setPicked(thread.id);
                  }}
                  onMouseEnter={() => {
                    setHovered(thread.id);
                  }}
                  onMouseLeave={() => {
                    setHovered((now) => (now === thread.id ? null : now));
                  }}
                >
                  <ThreadCard
                    thread={thread}
                    heading={on}
                    state={state}
                    source={source}
                    picked={picked === thread.id}
                    viewer={user?.name ?? null}
                    actions={
                      !mayComment ? null : thread.status === "open" ? (
                        <OpenActions
                          domain={domain}
                          path={path}
                          thread={thread}
                          onAnswered={refresh}
                          onResolved={afterMove}
                        />
                      ) : (
                        <ReopenAction
                          domain={domain}
                          path={path}
                          thread={thread}
                          onDone={afterMove}
                        />
                      )
                    }
                  />
                </div>
              ))}
            </div>
          );
        })}
      </div>
    </>
  );

  // A polite line for what happened. While the sheet is open it sits inside
  // the sheet, as the modal hides everything outside it from a screen
  // reader; while it is shut, outside it.
  const status = (
    <p role="status" className="sr-only">
      {said ?? ""}
    </p>
  );

  if (column) {
    return (
      <Kept.Provider value={kept}>
        <aside
          aria-label="Threads"
          className="flex flex-col gap-3 print:hidden"
        >
          {status}
          {body}
        </aside>
        {bar}
      </Kept.Provider>
    );
  }

  const pill =
    open.length === 0 ? "Threads" : plural(open.length, "thread", "threads");
  return (
    <Kept.Provider value={kept}>
      {!sheetOpen && status}
      {bar}
      <Dialog.Root open={sheetOpen} onOpenChange={setSheetOpen}>
        {/*
          A plain button rather than radix's trigger, whose code would land in
          the eager bundle beside the dialog the shell already uses; the
          sheet hands focus back to it by hand.
        */}
        <button
          ref={pillRef}
          type="button"
          aria-label={`Show ${open.length === 0 ? "threads" : pill}`}
          aria-haspopup="dialog"
          aria-expanded={sheetOpen}
          // Keep a selection on the page: the Comment button is in the
          // sheet, and it comments on what was selected.
          onMouseDown={(event) => {
            event.preventDefault();
          }}
          onClick={() => {
            setSheetOpen(true);
          }}
          className={`fixed bottom-5 left-1/2 z-30 inline-flex h-11 -translate-x-1/2 items-center gap-2 rounded-full bg-ink px-4 text-[15px] font-medium text-canvas shadow-pop print:hidden ${FOCUS_RING}`}
        >
          <MessagesSquare aria-hidden size={18} />
          {pill}
        </button>
        <Dialog.Portal>
          <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40 print:hidden" />
          <Dialog.Content
            aria-describedby={undefined}
            aria-modal="true"
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              // Back to the pill; when the column took over (the window grew
              // past the breakpoint) to the column's heading.
              (pillRef.current ?? threadsHeading.current)?.focus();
            }}
            // At the thread a tap or a link asked for, else at the heading:
            // the first control (the switch) is not where a reader starts.
            onOpenAutoFocus={(event) => {
              event.preventDefault();
              setOpenAt(null);
              const target =
                openAt === null ? null : document.getElementById(openAt);
              (target ?? threadsHeading.current)?.focus();
              target?.scrollIntoView?.({ block: "nearest" });
            }}
            className="fixed inset-x-0 bottom-0 z-50 mx-auto flex max-h-[85dvh] w-full max-w-xl flex-col rounded-t-2xl bg-raised shadow-pop outline-none print:hidden"
          >
            <div
              aria-hidden
              className="mx-auto mt-2 mb-1 h-1 w-10 shrink-0 rounded-full bg-hair-strong"
            />
            {status}
            <div className="flex min-h-0 flex-col gap-3 overflow-y-auto px-4 pt-1 pb-[max(1.5rem,env(safe-area-inset-bottom))]">
              {body}
            </div>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </Kept.Provider>
  );
}

/** A passage by value: its section, quote and occurrence. */
function passageKey(anchor: SelectionAnchor): string {
  return JSON.stringify([
    anchor.section,
    anchor.quote ?? null,
    anchor.occurrence ?? null,
  ]);
}

/** Where the comment in progress is kept while the sheet is shut. */
const COMMENT_KEY = "comment";

/** Where its label is kept. */
const COMMENT_LABEL_KEY = "comment-label";

function CommentForm({
  anchor,
  source,
  pending,
  error,
  onCancel,
  onSubmit,
}: {
  anchor: SelectionAnchor;
  source: string | null;
  pending: boolean;
  error: Error | null;
  onCancel: () => void;
  onSubmit: (text: string) => void;
}): ReactElement {
  const [text, setText] = useKept(COMMENT_KEY, "");
  const label = useLabel(COMMENT_LABEL_KEY, setText);
  const field = useId();
  const box = useRef<HTMLTextAreaElement | null>(null);
  const quote = quoteOf(anchor.quote);
  useEffect(() => {
    box.current?.focus();
  }, []);
  return (
    <form
      aria-label="New comment"
      className="flex flex-col gap-2 rounded-[10px] border border-hair p-3 text-sm"
      onSubmit={(event) => {
        event.preventDefault();
        // With a label, the words' first line is its subject: words are
        // needed either way.
        if (text.trim() !== "" && !pending) {
          onSubmit(composeComment(label.head, text));
        }
      }}
    >
      {quote !== null ? (
        <blockquote className="border-l-2 border-(--mark-rule) py-px pl-2.5 text-[13px] text-ink-3">
          {shownQuote(quote, anchor.occurrence ?? 1, source)}
        </blockquote>
      ) : (
        <p className="text-caption">
          This passage cannot be pinned exactly, so the comment goes on the
          section.
        </p>
      )}
      <label htmlFor={field} className="text-caption">
        Your comment
      </label>
      <LabelChip head={label.head} onRemove={label.remove} />
      <textarea
        ref={box}
        id={field}
        value={text}
        rows={3}
        onChange={(event) => {
          label.type(event.target.value);
        }}
        className="rounded border border-hair-strong bg-raised px-2 py-1"
      />
      {error && (
        <p role="alert" className={ALERT}>
          {problemDetail(error)}
        </p>
      )}
      <div className="flex gap-2">
        <LabelMenu
          head={label.head}
          onChange={label.setHead}
          onClosed={() => box.current?.focus()}
        />
        <button type="submit" disabled={pending} className={BUTTON.primary}>
          Comment
        </button>
        <button type="button" onClick={onCancel} className={BUTTON.ghost}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/**
 * One labelled field, its send button and Cancel, for a step that takes
 * words. The field takes focus when the form opens.
 */
function StepForm({
  keep,
  label,
  labelled = false,
  button,
  pending,
  error,
  onSend,
  onCancel,
}: {
  /** Where the words in progress are kept while the sheet is shut. */
  keep: string;
  label: string;
  /** Whether the words may carry a conventional comment label (a reply). */
  labelled?: boolean;
  button: string;
  pending: boolean;
  error: Error | null;
  /** Sends the words; the caller closes the form once they landed. */
  onSend: (text: string) => void;
  onCancel: () => void;
}): ReactElement {
  const [value, setValue] = useKept(keep, "");
  const head = useLabel(`${keep}-label`, setValue);
  const id = useId();
  const field = useRef<HTMLInputElement | null>(null);
  useEffect(() => {
    field.current?.focus();
  }, []);
  return (
    <form
      className="flex w-full flex-col gap-1"
      onSubmit={(event) => {
        event.preventDefault();
        if (value.trim() !== "" && !pending) {
          onSend(composeComment(labelled ? head.head : null, value));
        }
      }}
    >
      <label htmlFor={id} className="text-caption">
        {label}
      </label>
      {labelled && <LabelChip head={head.head} onRemove={head.remove} />}
      <input
        ref={field}
        id={id}
        value={value}
        onChange={(event) => {
          if (labelled) {
            head.type(event.target.value);
          } else {
            setValue(event.target.value);
          }
        }}
        className={`min-w-0 ${FIELD}`}
      />
      {error && (
        <p role="alert" className={ALERT}>
          {problemDetail(error)}
        </p>
      )}
      <div className="flex gap-2">
        {labelled && (
          <LabelMenu
            head={head.head}
            onChange={head.setHead}
            onClosed={() => field.current?.focus()}
          />
        )}
        <button type="submit" disabled={pending} className={BUTTON.secondary}>
          {button}
        </button>
        <button type="button" onClick={onCancel} className={BUTTON.ghost}>
          Cancel
        </button>
      </div>
    </form>
  );
}

type Step = "answer" | "resolve";

/**
 * The write actions of an open thread: a quiet row of two buttons, each
 * opening its own form on demand. One form is open at a time, and closing it
 * (Cancel or a send that landed) returns focus to the button that opened it.
 */
function OpenActions({
  domain,
  path,
  thread,
  onAnswered,
  onResolved,
}: {
  domain: string;
  path: string;
  thread: ThreadView;
  onAnswered: () => Promise<unknown>;
  onResolved: () => Promise<unknown>;
}): ReactElement {
  const [form, setForm] = useKept<Step | null>(`${thread.id}:form`, null);
  const forget = useForget();
  const answerButton = useRef<HTMLButtonElement | null>(null);
  const resolveButton = useRef<HTMLButtonElement | null>(null);
  // Set when a form closes, so focus goes back once its button is drawn again.
  const backTo = useRef<Step | null>(null);
  useEffect(() => {
    if (form === null && backTo.current !== null) {
      const button = backTo.current === "answer" ? answerButton : resolveButton;
      backTo.current = null;
      button.current?.focus();
    }
  }, [form]);
  const close = (step: Step) => {
    forget(`${thread.id}:${step}`);
    forget(`${thread.id}:${step}-label`);
    forget(`${thread.id}:${step}-label-removed`);
    backTo.current = step;
    setForm(null);
  };
  // The kept words go when the step lands, at the mutation rather than in
  // the call's own callback: a sheet shut in between unmounts this row, and
  // only the mutation's callbacks still run then.
  const answer = useMutation({
    mutationFn: (text: string) =>
      stepThread(domain, thread.id, "answer", { path, text }),
    onSuccess: () => {
      forget(`${thread.id}:answer`);
      forget(`${thread.id}:answer-label`);
      forget(`${thread.id}:answer-label-removed`);
      forget(`${thread.id}:form`);
      void onAnswered();
    },
  });
  const resolve = useMutation({
    mutationFn: (outcome: string) =>
      stepThread(domain, thread.id, "resolve", { path, outcome }),
    onSuccess: () => {
      forget(`${thread.id}:resolve`);
      forget(`${thread.id}:form`);
      void onResolved();
    },
  });
  const openForm = (step: Step) => {
    answer.reset();
    resolve.reset();
    setForm(step);
  };
  if (form === "answer") {
    return (
      <StepForm
        keep={`${thread.id}:answer`}
        label="Your answer"
        labelled
        button="Answer"
        pending={answer.isPending}
        error={answer.error}
        onSend={(text) => {
          answer.mutate(text, {
            onSuccess: () => {
              close("answer");
            },
          });
        }}
        onCancel={() => {
          close("answer");
        }}
      />
    );
  }
  if (form === "resolve") {
    return (
      <StepForm
        keep={`${thread.id}:resolve`}
        label="Outcome"
        button="Resolve"
        pending={resolve.isPending}
        error={resolve.error}
        onSend={(text) => {
          resolve.mutate(text, {
            onSuccess: () => {
              // The thread moves out of this list, so focus goes to the
              // column's heading (set by `onResolved`), not back here.
              setForm(null);
            },
          });
        }}
        onCancel={() => {
          close("resolve");
        }}
      />
    );
  }
  return (
    <div className="flex gap-4">
      <button
        ref={answerButton}
        type="button"
        onClick={() => {
          openForm("answer");
        }}
        className={QUIET}
      >
        Answer
      </button>
      <button
        ref={resolveButton}
        type="button"
        onClick={() => {
          openForm("resolve");
        }}
        className={QUIET}
      >
        Resolve
      </button>
    </div>
  );
}

function ReopenAction({
  domain,
  path,
  thread,
  onDone,
}: {
  domain: string;
  path: string;
  thread: ThreadView;
  onDone: () => Promise<unknown>;
}): ReactElement {
  const reopen = useMutation({
    mutationFn: () => stepThread(domain, thread.id, "reopen", { path }),
    onSuccess: () => {
      void onDone();
    },
  });
  return (
    <>
      <button
        type="button"
        disabled={reopen.isPending}
        onClick={() => {
          reopen.mutate();
        }}
        className={QUIET}
      >
        Reopen
      </button>
      {reopen.error && (
        <p role="alert" className={`w-full ${ALERT}`}>
          {problemDetail(reopen.error)}
        </p>
      )}
    </>
  );
}
