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
 */

import type { EditorView } from "@codemirror/view";
import {
  AlignJustify,
  Bold,
  Brackets,
  Code,
  Columns3,
  Grid2X2X,
  Heading,
  Italic,
  Link,
  List,
  ListOrdered,
  ListTodo,
  Rows3,
  SquareCode,
  Strikethrough,
  TextQuote,
  Trash2,
  WandSparkles,
  Workflow,
} from "lucide-react";
import { DropdownMenu } from "radix-ui";
import type { MouseEvent, ReactElement } from "react";

import { ITEM_CLASSES, MENU_CLASSES } from "../components/menu";
import { IconButton } from "../components/primitives";
import { TableSizePicker } from "./TableSizePicker";
import type { Align } from "./tableModel";
import {
  CODE_SKELETON,
  MERMAID_SKELETON,
  ORDERED_ITEM,
  cycleHeading,
  insertBlock,
  insertMarkdownLink,
  insertWikilink,
  toggleInline,
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

/** The heading levels the menu offers - deeper marks stay a typed thing. */
const HEADING_LEVELS = [1, 2, 3];

/** The alignments a column can be given, in the order they read. */
const ALIGNMENTS: { align: Align; label: string }[] = [
  { align: "left", label: "Align left" },
  { align: "center", label: "Align center" },
  { align: "right", label: "Align right" },
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

export function EditorToolbar({
  view,
  tableActive = false,
}: {
  view: EditorView | null;
  /** Whether the caret is in a table right now; the screen watches for it. */
  tableActive?: boolean;
}): ReactElement {
  const off = view === null;
  const act = (run: (view: EditorView) => boolean) => () => {
    if (view) {
      run(view);
    }
  };
  const keepSelection = (event: MouseEvent) => {
    event.preventDefault();
  };
  return (
    <div
      role="toolbar"
      aria-label="Formatting"
      onMouseDown={keepSelection}
      className="sticky top-14 z-10 flex flex-wrap items-center gap-0.5 rounded-t border-b border-slate-200 bg-white/95 px-1.5 py-1 backdrop-blur print:hidden dark:border-slate-800 dark:bg-slate-950/95"
    >
      <IconButton
        label="Bold"
        icon={Bold}
        disabled={off}
        onClick={act((v) => toggleInline(v, "**"))}
      />
      <IconButton
        label="Italic"
        icon={Italic}
        disabled={off}
        onClick={act((v) => toggleInline(v, "*"))}
      />
      <IconButton
        label="Strikethrough"
        icon={Strikethrough}
        disabled={off}
        onClick={act((v) => toggleInline(v, "~~"))}
      />
      <IconButton
        label="Inline code"
        icon={Code}
        disabled={off}
        onClick={act((v) => toggleInline(v, "`"))}
      />
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <IconButton label="Heading" icon={Heading} disabled={off} />
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="start"
            sideOffset={6}
            className={MENU_CLASSES}
            // Radix hands focus back to the trigger when the menu closes,
            // which would leave the caret out of the buffer right after a
            // command put it back there. The buffer is where an author wants
            // to be either way - having picked a heading, or having changed
            // their mind and pressed Escape - so this bar always returns them.
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              view?.focus();
            }}
          >
            {HEADING_LEVELS.map((level) => (
              <DropdownMenu.Item
                key={level}
                className={ITEM_CLASSES}
                onSelect={act((v) => cycleHeading(v, level))}
              >
                Heading {level}
              </DropdownMenu.Item>
            ))}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
      <IconButton
        label="Bulleted list"
        icon={List}
        disabled={off}
        onClick={act((v) => toggleLinePrefix(v, "- "))}
      />
      <IconButton
        label="Numbered list"
        icon={ListOrdered}
        disabled={off}
        onClick={act((v) => toggleLinePrefix(v, "1. ", ORDERED_ITEM))}
      />
      <IconButton
        label="Task list"
        icon={ListTodo}
        disabled={off}
        onClick={act((v) => toggleLinePrefix(v, "- [ ] "))}
      />
      <IconButton
        label="Blockquote"
        icon={TextQuote}
        disabled={off}
        onClick={act((v) => toggleLinePrefix(v, "> "))}
      />
      <IconButton
        label="Wiki link"
        icon={Brackets}
        disabled={off}
        onClick={act(insertWikilink)}
      />
      <IconButton
        label="Link"
        icon={Link}
        disabled={off}
        onClick={act(insertMarkdownLink)}
      />
      {/*
        The insert verbs are named buttons of their own, and this hairline
        only says they are a different kind of thing.
      */}
      <Divider />
      {/*
        The one insert verb that asks a question first: how big. Its trigger
        is this bar's own icon button, so the row reads as one kind of thing
        whatever happens after the press.
      */}
      <TableSizePicker view={view} />
      <IconButton
        label="Insert diagram"
        icon={Workflow}
        disabled={off}
        onClick={act((v) => insertBlock(v, MERMAID_SKELETON))}
      />
      <IconButton
        label="Insert code block"
        icon={SquareCode}
        disabled={off}
        onClick={act((v) => insertBlock(v, CODE_SKELETON))}
      />
      {tableActive && (
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
            icon={Trash2}
            disabled={off}
            onClick={act(tableDeleteRow)}
          />
          <IconButton
            label="Delete column"
            icon={Grid2X2X}
            disabled={off}
            onClick={act(tableDeleteColumn)}
          />
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <IconButton
                label="Align column"
                icon={AlignJustify}
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
                {ALIGNMENTS.map(({ align, label }) => (
                  <DropdownMenu.Item
                    key={align}
                    className={ITEM_CLASSES}
                    onSelect={act((v) => tableAlignColumn(v, align))}
                  >
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
    </div>
  );
}
