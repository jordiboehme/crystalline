/**
 * The floating format bar: markdown for people who do not remember markdown.
 * Sticky under the h-14 top bar at the head of the editor card, live in both
 * preview and Raw mode - it edits source text, which both modes are.
 *
 * Every button is an ordinary tab stop rather than a roving-tabindex group:
 * they are all named, and native buttons already answer Enter and Space. What
 * they must not do is take the selection away from the buffer they are about
 * to edit, so the bar cancels its own mousedown - the event that would move
 * focus - and every command puts the caret back in the buffer when it is done.
 *
 * The last segment is context rather than chrome: the table verbs are drawn
 * only while the caret is actually in a table, because they have nothing to
 * act on anywhere else and six permanently disabled buttons would be six
 * things to read past. What arrives from the screen is a single boolean; every
 * verb re-derives the table from the live state when it runs, so a click that
 * beats a stale render refuses instead of editing the wrong place.
 *
 * Both editors mount this bar, and what differs between their documents comes
 * in as a document adapter. The bar asks it before a heading change: on a
 * Reflection page, a section heading that stops being one joins its section
 * into the one above, and the bar says so in one plain line and waits for the
 * person to confirm.
 */

import type { EditorView } from "@codemirror/view";
import {
  Bold,
  Brackets,
  Check,
  ChevronDown,
  Code,
  Columns3,
  Italic,
  Link,
  List,
  ListOrdered,
  ListTodo,
  Rows3,
  Strikethrough,
  TextQuote,
  WandSparkles,
} from "lucide-react";
import { AlertDialog, DropdownMenu } from "radix-ui";
import type { MouseEvent, ReactElement, ReactNode } from "react";
import { useRef, useState } from "react";

import { CALLOUT_WORDS } from "../components/calloutFace";
import { ITEM_CLASSES, MENU_CLASSES, MENU_SEPARATOR } from "../components/menu";
import { BUTTON, FOCUS_RING, IconButton } from "../components/primitives";
import type { CalloutKind } from "../markdown/callouts";
import { CALLOUT_KINDS } from "../markdown/callouts";
import type { StatusColour } from "../markdown/directives";
import { STATUS_COLOURS } from "../markdown/directives";
import type { IconComponent } from "../components/primitives";
import type { DocumentAdapter } from "./adapter";
import { AttachButton } from "./AttachButton";
import {
  setCalloutFold,
  setCalloutKind,
  setStatusColour,
} from "./elementContext";
import { verbTarget } from "./floatingContext";
import type { Format } from "./marks/formatting";
import { toggleFormat } from "./marks/formatting";
import { mainSelectionParked } from "./parkedSelection";
import { stepOffIdLines } from "./hiddenIdLines";
import { InsertMenu, TRIGGER } from "./InsertMenu";
import type { StyleLevel } from "./TextStyleMenu";
import { TextStyleMenu } from "./TextStyleMenu";
import { TableSizePicker } from "./TableSizePicker";
import type { ImageAlign } from "./imageFormat";
import {
  ImageCenterIcon,
  ImageFloatLeftIcon,
  ImageFloatRightIcon,
  ImageFormatMenuIcon,
  ImageFullIcon,
  ImageWidth25Icon,
  ImageWidth50Icon,
  ImageWidth75Icon,
} from "./imageIcons";
import { clearImageFormat, setImageAlign, setImageWidth } from "./imageVerbs";
import {
  AlignColumnCenterIcon,
  AlignColumnIcon,
  AlignColumnLeftIcon,
  AlignColumnRightIcon,
  DeleteColumnIcon,
  DeleteRowIcon,
} from "./tableIcons";
import type { Align } from "./tableModel";
import {
  ORDERED_ITEM,
  setHeadingLevel,
  insertMarkdownLink,
  insertWikilink,
  toggleLinePrefix,
} from "./toolbar";
import {
  tableAddColumnAfter,
  tableAddRowBelow,
  tableAlignColumn,
  tableDeleteColumn,
  tableDeleteRow,
  tablePrettify,
} from "./tableVerbs";

/**
 * The alignments a column can be given, in the order they read, each with the
 * mark that says it without being read: the rows of a menu are icons in every
 * other editor an author has used.
 *
 * Hand drawn rather than borrowed, because what these set is a table column
 * and the set's own alignment marks are lines of a paragraph: the same three
 * lines here stand inside the column frame the delete glyphs are built on. The
 * trigger above them is framed the same way and points both ways at once,
 * where it used to wear the centre mark and so read as one of its own answers.
 */
const ALIGNMENTS: { align: Align; label: string; icon: IconComponent }[] = [
  { align: "left", label: "Align left", icon: AlignColumnLeftIcon },
  { align: "center", label: "Align center", icon: AlignColumnCenterIcon },
  { align: "right", label: "Align right", icon: AlignColumnRightIcon },
];

/**
 * Where an image may stand, in the order they read, each with the mark that
 * says it without being read.
 *
 * Centered leads because it is the default an upload writes and the answer
 * that clears the fragment again: an author who tried a float and changed
 * their mind finds the way back at the top of the menu rather than having to
 * know that "no fragment" is a state.
 */
const PLACEMENTS: { align: ImageAlign; label: string; icon: IconComponent }[] =
  [
    { align: "center", label: "Centered", icon: ImageCenterIcon },
    { align: "full", label: "Full width", icon: ImageFullIcon },
    { align: "left", label: "Float left", icon: ImageFloatLeftIcon },
    { align: "right", label: "Float right", icon: ImageFloatRightIcon },
  ];

/**
 * The widths the menu offers, as shares of the column.
 *
 * Presets rather than a number field: the point of the menu is to place a
 * screenshot in two clicks, and a picture whose size actually matters is
 * better served by the fragment written by hand. Each composes with whatever
 * placement the image already has, so the two halves of the menu never undo
 * each other.
 */
const WIDTHS: { width: string; label: string; icon: IconComponent }[] = [
  { width: "25%", label: "Width 25%", icon: ImageWidth25Icon },
  { width: "50%", label: "Width 50%", icon: ImageWidth50Icon },
  { width: "75%", label: "Width 75%", icon: ImageWidth75Icon },
];

/** Decoration, not information: the same hairline every menu in the app draws. */
function Divider(): ReactElement {
  return (
    <span
      aria-hidden="true"
      className="mx-1 h-4 w-px bg-slate-200 dark:bg-slate-700"
    />
  );
}

/** The word each palette colour is shown and named as. */
const COLOUR_WORDS: Record<StatusColour, string> = {
  grey: "Grey",
  blue: "Blue",
  green: "Green",
  yellow: "Yellow",
  red: "Red",
  purple: "Purple",
};

/** A segmented control's track and its segments, the avatar menu's look. */
const TRACK = "inline-flex shrink-0 gap-0.5 rounded-[9px] bg-seg p-0.5";
const SEGMENT = `inline-flex h-7 shrink-0 cursor-pointer items-center gap-1.5 rounded-[7px] px-2 text-[13px] text-muted hover:text-ink disabled:cursor-default disabled:opacity-50 aria-pressed:bg-seg-on aria-pressed:text-ink aria-pressed:shadow-[0_1px_2px_rgb(15_23_42/0.1)] ${FOCUS_RING}`;

/** What the bar knows about the callout the caret is in. */
export interface CalloutFace {
  callout: CalloutKind;
  fold: "open" | "closed" | null;
}

/**
 * The callout segment: its kind, as a menu that reads like a select, and
 * whether it starts open, starts closed or does not fold. A details callout
 * always folds (with no sign it is closed, as the page draws it), so it has
 * no "Does not fold".
 */
function CalloutSegment({
  face,
  view,
  allow,
}: {
  face: CalloutFace;
  view: EditorView | null;
  /** Asked when a change runs: false refuses it (main's turn or not). */
  allow: (view: EditorView) => boolean;
}): ReactElement {
  const word = CALLOUT_WORDS[face.callout];
  const fold = face.fold ?? (face.callout === "details" ? "closed" : null);
  const run = (edit: (v: EditorView) => boolean) => {
    if (view && allow(view)) {
      edit(view);
    }
  };
  return (
    <>
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          {/* The visible word first, then what it is (label in name). */}
          <button
            type="button"
            className={TRIGGER}
            disabled={view === null}
            aria-label={`${word}, callout kind`}
          >
            {word}
            <ChevronDown aria-hidden="true" size={14} strokeWidth={1.75} />
          </button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="start"
            sideOffset={6}
            className={MENU_CLASSES}
            // The bar's discipline: back to the buffer when the menu closes.
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              view?.focus();
            }}
          >
            <DropdownMenu.RadioGroup
              value={face.callout}
              onValueChange={(next) => {
                run((v) => setCalloutKind(v, next as CalloutKind));
              }}
            >
              {CALLOUT_KINDS.map((kind) => (
                <DropdownMenu.RadioItem
                  key={kind}
                  value={kind}
                  className={ITEM_CLASSES}
                >
                  <span className="inline-flex w-3.5 shrink-0">
                    <DropdownMenu.ItemIndicator>
                      <Check aria-hidden="true" size={14} strokeWidth={2} />
                    </DropdownMenu.ItemIndicator>
                  </span>
                  {CALLOUT_WORDS[kind]}
                </DropdownMenu.RadioItem>
              ))}
            </DropdownMenu.RadioGroup>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
      <div role="group" aria-label="Callout fold" className={TRACK}>
        <button
          type="button"
          className={SEGMENT}
          disabled={view === null}
          aria-pressed={fold === "open"}
          onClick={() => {
            run((v) => setCalloutFold(v, "open"));
          }}
        >
          Starts open
        </button>
        <button
          type="button"
          className={SEGMENT}
          disabled={view === null}
          aria-pressed={fold === "closed"}
          onClick={() => {
            run((v) => setCalloutFold(v, "closed"));
          }}
        >
          Starts closed
        </button>
        {face.callout === "details" ? null : (
          <button
            type="button"
            className={SEGMENT}
            disabled={view === null}
            aria-pressed={fold === null}
            onClick={() => {
              run((v) => setCalloutFold(v, null));
            }}
          >
            Does not fold
          </button>
        )}
      </div>
    </>
  );
}

/**
 * The status segment: the six palette colours, each named by its word with
 * a dot of its colour beside it, the label's own pressed.
 */
function StatusSegment({
  colour,
  view,
  allow,
}: {
  colour: StatusColour;
  view: EditorView | null;
  /** Asked when a change runs: false refuses it (main's turn or not). */
  allow: (view: EditorView) => boolean;
}): ReactElement {
  return (
    <div role="group" aria-label="Status colour" className={TRACK}>
      {STATUS_COLOURS.map((each) => (
        <button
          key={each}
          type="button"
          className={SEGMENT}
          disabled={view === null}
          aria-pressed={each === colour}
          onClick={() => {
            if (view && allow(view)) {
              setStatusColour(view, each);
            }
          }}
        >
          <span
            aria-hidden="true"
            className="inline-block size-2 shrink-0 rounded-full"
            style={{ background: `var(--status-${each})` }}
          />
          {COLOUR_WORDS[each]}
        </button>
      ))}
    </div>
  );
}

/**
 * The one question a heading change can ask, in the shape of the other
 * dialogs in the editor (`ConfirmLeaveDialog`): the sentence the adapter
 * answered, the change as the primary answer, Cancel quiet. Escape and the
 * scrim mean Cancel.
 */
function ConfirmHeadingDialog({
  sentence,
  onConfirm,
  onCancel,
  onClosed,
}: {
  sentence: string;
  onConfirm: () => void;
  onCancel: () => void;
  /** Where focus goes once the dialog has closed: back to the buffer. */
  onClosed: () => void;
}): ReactElement {
  return (
    <AlertDialog.Root
      open
      onOpenChange={(next) => {
        if (!next) {
          onCancel();
        }
      }}
    >
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <AlertDialog.Content
          className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900"
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            onClosed();
          }}
        >
          <AlertDialog.Title className="text-lg font-semibold">
            Join sections?
          </AlertDialog.Title>
          <AlertDialog.Description className="mt-1 text-sm text-slate-600 dark:text-slate-300">
            {sentence}
          </AlertDialog.Description>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <AlertDialog.Cancel className={BUTTON.ghost}>
              Cancel
            </AlertDialog.Cancel>
            <AlertDialog.Action className={BUTTON.primary} onClick={onConfirm}>
              Join sections
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

export function EditorToolbar({
  view: buffer,
  tableActive = false,
  tableDrawn = false,
  raw = false,
  imageActive = false,
  calloutActive = null,
  statusActive = null,
  onAttach,
  bare = false,
  trailing,
  adapter,
  disabled = false,
  attachDisabled = false,
  cellOpen = false,
  formats,
}: {
  view: EditorView | null;
  /** Whether the caret is in a table right now; the screen watches for it. */
  tableActive?: boolean;
  /**
   * Whether the table grid draws the table the caret is in. A drawn table's
   * add, delete, align and prettify verbs live in the grid's cell menu and
   * floating actions, so the bar's table segment steps aside for it.
   */
  tableDrawn?: boolean;
  /**
   * Raw mode: the table is pipe source again and the grid is gone, so the
   * segment is drawn whatever `tableDrawn` says.
   */
  raw?: boolean;
  /**
   * Whether the caret is on an attachment image right now. Drawn rather than
   * disabled, like the table segment beside it: a menu that has nothing to act
   * on is one more thing to read past on the way to the verbs that do.
   */
  imageActive?: boolean;
  /**
   * The callout the caret is in right now, or null: its kind and fold, for
   * the segment that changes them. Drawn rather than disabled, like the
   * table and image segments.
   */
  calloutActive?: CalloutFace | null;
  /** The colour of the status label the caret is in right now, or null. */
  statusActive?: StatusColour | null;
  /**
   * What to do with the files a person picks, on a surface that can hold
   * attachments. Absent on one that cannot - the MANIFEST - where the button
   * is simply not drawn rather than drawn and refusing.
   */
  onAttach?: (files: File[]) => void;
  /**
   * The reading-column face: a sticky row with a hairline under it and no box
   * around it, one line that scrolls sideways on a narrow screen rather than
   * wrapping. The default is the boxed bar the MANIFEST editor still wears.
   */
  bare?: boolean;
  /** Drawn at the row's right end, past the verbs: the engram editor's Raw. */
  trailing?: ReactNode;
  /**
   * What this kind of document adds, asked before a heading change. Absent
   * on the MANIFEST, whose headings never ask.
   */
  adapter?: DocumentAdapter;
  /** Every verb off: a Reflection room that has ended takes no edit. */
  disabled?: boolean;
  /**
   * Attach alone off: a Reflection room that is away takes typing, which it
   * merges on its return, but a person should not start an upload the page
   * cannot take now.
   */
  attachDisabled?: boolean;
  /**
   * Whether a table cell is open in the grid. Main's selection is stale
   * then: the inline verbs (marks and links) run in the cell, and the verbs
   * that act on main's line or insert a block there are off, as are the
   * segments drawn from main's caret (image, table, callout, status).
   */
  cellOpen?: boolean;
  /**
   * The formats the view a format verb acts on has at its caret or over its
   * selection, drawn as Bold, Italic, Strikethrough and Inline code pressed.
   * The screen is told them by the buffer's format state listener. Absent
   * (a surface with no listener, the MANIFEST), the buttons are plain
   * buttons rather than toggles that would always say "not pressed".
   */
  formats?: ReadonlySet<Format>;
}): ReactElement {
  const view = disabled ? null : buffer;
  const off = view === null;
  /**
   * Main's selection parked behind an open cell whose editor is not mounted
   * (its table scrolled away), with main not holding the focus: the screen's
   * `cellOpen` is false then, but main's selection is as stale.
   */
  const parked =
    view !== null &&
    view.state.facet(mainSelectionParked) &&
    view.dom.ownerDocument.activeElement !== view.contentDOM;
  const cellNow = cellOpen || parked;
  /** Off for the verbs that act on main's line or insert into main. */
  const lineOff = off || cellNow;
  /** The heading change waiting for the person's answer, if any. */
  const [asking, setAsking] = useState<{
    level: StyleLevel;
    sentence: string;
  } | null>(null);
  /**
   * Set as the heading menu picks a change that asks first, so the menu's
   * close hands focus to the dialog rather than back to the buffer. A ref,
   * because the menu closes before the state above has rendered.
   */
  const askingNow = useRef(false);
  /**
   * Whether a verb on main's line or caret may run now. The view a verb acts
   * on is `verbTarget`'s, read when it runs: the open cell's editor, else
   * main, or nothing while main's selection is parked behind a cell whose
   * editor is not mounted. The floating bar uses the same rule.
   */
  const mainTurn = (v: EditorView): boolean => verbTarget(v) === v;
  /**
   * A verb on main's line or main's caret. Refused unless main is the
   * target, so a click that beats the render that turned the button off
   * still changes nothing.
   */
  const act = (run: (view: EditorView) => boolean) => () => {
    if (view && verbTarget(view) === view) {
      run(view);
    }
  };
  /** An inline verb: in the open cell when there is one, else on main. */
  const actInline = (run: (view: EditorView) => boolean) => () => {
    const target = view ? verbTarget(view) : null;
    if (target) {
      run(target);
    }
  };
  const keepSelection = (event: MouseEvent) => {
    event.preventDefault();
  };
  /** A format button's pressed state, when the screen tells the formats. */
  const pressed = (format: Format) =>
    formats ? { "aria-pressed": formats.has(format) } : {};
  /** The question a heading change on the caret's line asks, or null. */
  const questionFor = (v: EditorView, level: StyleLevel): string | null => {
    const line = v.state.doc.lineAt(v.state.selection.main.head).number;
    return adapter?.confirmHeadingChange(v.state, line, level) ?? null;
  };
  const pickHeading = (v: EditorView, level: StyleLevel): boolean => {
    // The line the change is made on, as `setHeadingLevel` will find it: off a
    // hidden id line first, so the question is about that same line.
    stepOffIdLines(v);
    const sentence = questionFor(v, level);
    if (sentence === null) {
      return setHeadingLevel(v, level);
    }
    askingNow.current = true;
    setAsking({ level, sentence });
    return true;
  };
  const answered = () => {
    askingNow.current = false;
    setAsking(null);
  };
  const joinSections = () => {
    const level = asking?.level;
    answered();
    // Asked again against the live text: in a room the lines may have moved
    // while the dialog was open, and a line that no longer asks is left as
    // it is.
    if (view && level !== undefined && questionFor(view, level) !== null) {
      setHeadingLevel(view, level, { joinSections: true });
    }
  };
  return (
    <>
      {/*
        On the reading column the bar is one row that scrolls on a phone and
        wraps from md up, so a context segment (a callout's kind and fold, a
        label's colours) stands in view where the caret asks for it.
      */}
      <div
        role="toolbar"
        aria-label="Formatting"
        onMouseDown={keepSelection}
        className={
          bare
            ? "sticky top-14 z-10 flex flex-nowrap items-center gap-0.5 overflow-x-auto border-b border-hair bg-canvas/95 py-1.5 backdrop-blur md:flex-wrap md:overflow-x-visible print:hidden"
            : "sticky top-14 z-10 flex flex-wrap items-center gap-0.5 rounded-t border-b border-slate-200 bg-white/95 px-1.5 py-1 backdrop-blur print:hidden dark:border-slate-800 dark:bg-slate-950/95"
        }
      >
        <IconButton
          label="Bold"
          icon={Bold}
          disabled={off}
          {...pressed("strong")}
          onClick={actInline((v) => toggleFormat(v, "strong"))}
        />
        <IconButton
          label="Italic"
          icon={Italic}
          disabled={off}
          {...pressed("em")}
          onClick={actInline((v) => toggleFormat(v, "em"))}
        />
        <IconButton
          label="Strikethrough"
          icon={Strikethrough}
          disabled={off}
          {...pressed("strike")}
          onClick={actInline((v) => toggleFormat(v, "strike"))}
        />
        <IconButton
          label="Inline code"
          icon={Code}
          disabled={off}
          {...pressed("code")}
          onClick={actInline((v) => toggleFormat(v, "code"))}
        />
        <TextStyleMenu
          view={view}
          disabled={lineOff}
          keepsFocus={() => askingNow.current}
          onPick={(level) => {
            if (view && verbTarget(view) === view) {
              pickHeading(view, level);
            }
          }}
        />
        <IconButton
          label="Bulleted list"
          icon={List}
          disabled={lineOff}
          onClick={act((v) => toggleLinePrefix(v, "- "))}
        />
        <IconButton
          label="Numbered list"
          icon={ListOrdered}
          disabled={lineOff}
          onClick={act((v) => toggleLinePrefix(v, "1. ", ORDERED_ITEM))}
        />
        <IconButton
          label="Task list"
          icon={ListTodo}
          disabled={lineOff}
          onClick={act((v) => toggleLinePrefix(v, "- [ ] "))}
        />
        <IconButton
          label="Blockquote"
          icon={TextQuote}
          disabled={lineOff}
          onClick={act((v) => toggleLinePrefix(v, "> "))}
        />
        {/* Only where the document links engrams: a Reflection page does not. */}
        {(adapter?.wikilinks ?? true) && (
          <IconButton
            label="Wiki link"
            icon={Brackets}
            disabled={off}
            onClick={actInline(insertWikilink)}
          />
        )}
        <IconButton
          label="Link"
          icon={Link}
          disabled={off}
          onClick={actInline(insertMarkdownLink)}
        />
        {/*
        The insert verbs are named buttons of their own, and this hairline
        only says they are a different kind of thing.
      */}
        <Divider />
        {/*
        An insert verb that asks a question first: how big. Its trigger is
        this bar's own icon button, so the row reads as one kind of thing
        whatever happens after the press.
      */}
        <TableSizePicker view={lineOff ? null : view} allow={mainTurn} />
        {/*
        Every other element, as words: callouts, diagrams, blocks and inline
        elements, from the same registry as the slash menu.
      */}
        <InsertMenu view={lineOff ? null : view} allow={mainTurn} />
        {/*
        The other insert verb that asks first, and the only one whose question
        the browser asks: which file. Paste and drop reach the same flow with
        no button at all, so this is the way in for a file that is neither on
        the clipboard nor draggable onto the buffer.
      */}
        {onAttach && (
          <AttachButton
            onAttach={(files) => {
              // An upload inserts at main's selection: only on main's turn.
              if (view && mainTurn(view)) onAttach(files);
            }}
            disabled={lineOff || attachDisabled}
          />
        )}
        {imageActive && !cellNow && (
          <>
            {/*
            The same hairline, in front of the one menu that acts on the image
            the caret is on rather than on the document at large.
          */}
            <Divider />
            <DropdownMenu.Root>
              <DropdownMenu.Trigger asChild>
                <IconButton
                  label="Image format"
                  icon={ImageFormatMenuIcon}
                  disabled={off}
                />
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content
                  align="start"
                  sideOffset={6}
                  className={MENU_CLASSES}
                  // The heading menu's discipline, for the same reason: Radix
                  // hands focus back to the trigger on close, and this bar
                  // always returns an author to the buffer instead.
                  onCloseAutoFocus={(event) => {
                    event.preventDefault();
                    view?.focus();
                  }}
                >
                  {PLACEMENTS.map(({ align, label, icon: Icon }) => (
                    <DropdownMenu.Item
                      key={align}
                      className={ITEM_CLASSES}
                      onSelect={act((v) =>
                        // Centered is the default, and picking it means "as it
                        // was": the whole fragment goes, width included.
                        align === "center"
                          ? clearImageFormat(v)
                          : setImageAlign(v, align),
                      )}
                    >
                      <Icon aria-hidden="true" size={16} strokeWidth={1.75} />
                      {label}
                    </DropdownMenu.Item>
                  ))}
                  <DropdownMenu.Separator className={MENU_SEPARATOR} />
                  {WIDTHS.map(({ width, label, icon: Icon }) => (
                    <DropdownMenu.Item
                      key={width}
                      className={ITEM_CLASSES}
                      onSelect={act((v) => setImageWidth(v, width))}
                    >
                      <Icon aria-hidden="true" size={16} strokeWidth={1.75} />
                      {label}
                    </DropdownMenu.Item>
                  ))}
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
          </>
        )}
        {tableActive && !cellNow && (raw || !tableDrawn) && (
          <>
            {/*
            The same hairline again, in front of the verbs that act on the
            table the caret is in rather than on the document at large.
          */}
            <Divider />
            {/*
            The glyphs are a mnemonic; the labels are the contract. Every one
            of these is a tooltip as well as an accessible name, because a row
            of small squares is not self-explanatory whatever is drawn in it.

            All four say both halves now: what happens, and to which axis. The
            add verbs draw the axis from the shared set (Rows3, Columns3); the
            deletes draw the same three slots with the middle one crossed out,
            because the set has no such pair and the two used to share a trash
            can that could only say "something is destroyed". The axis was the
            half that went missing, and it was the half only the label carried.
          */}
            <IconButton
              label="Add row below"
              icon={Rows3}
              disabled={off}
              onClick={act(tableAddRowBelow)}
            />
            <IconButton
              label="Add column after"
              icon={Columns3}
              disabled={off}
              onClick={act(tableAddColumnAfter)}
            />
            <IconButton
              label="Delete row"
              icon={DeleteRowIcon}
              disabled={off}
              onClick={act(tableDeleteRow)}
            />
            <IconButton
              label="Delete column"
              icon={DeleteColumnIcon}
              disabled={off}
              onClick={act(tableDeleteColumn)}
            />
            <DropdownMenu.Root>
              <DropdownMenu.Trigger asChild>
                <IconButton
                  label="Align column"
                  icon={AlignColumnIcon}
                  disabled={off}
                />
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content
                  align="start"
                  sideOffset={6}
                  className={MENU_CLASSES}
                  // The heading menu's discipline, for the same reason: Radix
                  // hands focus back to the trigger on close, and this bar
                  // always returns an author to the buffer instead - having
                  // picked an alignment, or having pressed Escape.
                  onCloseAutoFocus={(event) => {
                    event.preventDefault();
                    view?.focus();
                  }}
                >
                  {ALIGNMENTS.map(({ align, label, icon: Icon }) => (
                    <DropdownMenu.Item
                      key={align}
                      className={ITEM_CLASSES}
                      onSelect={act((v) => tableAlignColumn(v, align))}
                    >
                      {/*
                      Decoration beside the label rather than a second name:
                      the row is called what it was always called, and the
                      glyph is the app's own icon idiom at the size every
                      other one is drawn at.
                    */}
                      <Icon aria-hidden="true" size={16} strokeWidth={1.75} />
                      {label}
                    </DropdownMenu.Item>
                  ))}
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
            <IconButton
              label="Prettify table"
              icon={WandSparkles}
              disabled={off}
              onClick={act(tablePrettify)}
            />
          </>
        )}
        {calloutActive !== null && !cellNow && (
          <>
            {/*
            The same hairline, in front of what acts on the callout the caret
            is in: its kind and whether it starts open.
          */}
            <Divider />
            <CalloutSegment face={calloutActive} view={view} allow={mainTurn} />
          </>
        )}
        {statusActive !== null && !cellNow && (
          <>
            <Divider />
            <StatusSegment colour={statusActive} view={view} allow={mainTurn} />
          </>
        )}
        {trailing !== undefined && (
          <div className="ml-auto flex shrink-0 items-center pl-2">
            {trailing}
          </div>
        )}
      </div>
      {/*
        Outside the row: its mousedown guard is for the verbs, and React hands
        a portal's events to the tree it was rendered in.
      */}
      {asking !== null && (
        <ConfirmHeadingDialog
          sentence={asking.sentence}
          onConfirm={joinSections}
          onCancel={answered}
          onClosed={() => {
            view?.focus();
          }}
        />
      )}
    </>
  );
}
