/**
 * The floating format bar: markdown for people who do not remember markdown.
 * Sticky under the h-14 top bar at the head of the editor card, live in both
 * preview and Raw mode - it edits source text, which both modes are.
 *
 * Every button is an ordinary tab stop rather than a roving-tabindex group:
 * there are nine of them, they are all named, and native buttons already
 * answer Enter and Space. What they must not do is take the selection away
 * from the buffer they are about to edit, so the bar cancels its own mousedown
 * - the event that would move focus - and every command puts the caret back in
 * the buffer when it is done.
 */

import type { EditorView } from "@codemirror/view";
import {
  Bold,
  Brackets,
  Code,
  Heading,
  Italic,
  List,
  ListTodo,
  Table,
  Workflow,
} from "lucide-react";
import { DropdownMenu } from "radix-ui";
import type { MouseEvent, ReactElement } from "react";

import { ITEM_CLASSES, MENU_CLASSES } from "../components/menu";
import { IconButton } from "../components/primitives";
import {
  MERMAID_SKELETON,
  TABLE_SKELETON,
  cycleHeading,
  insertBlock,
  insertWikilink,
  toggleInline,
  toggleLinePrefix,
} from "./toolbar";

/** The heading levels the menu offers - deeper marks stay a typed thing. */
const HEADING_LEVELS = [1, 2, 3];

export function EditorToolbar({
  view,
}: {
  view: EditorView | null;
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
        label="Task list"
        icon={ListTodo}
        disabled={off}
        onClick={act((v) => toggleLinePrefix(v, "- [ ] "))}
      />
      <IconButton
        label="Wiki link"
        icon={Brackets}
        disabled={off}
        onClick={act(insertWikilink)}
      />
      {/*
        Decoration, not information: the two insert verbs are named buttons
        of their own, and this hairline only says they are a different kind of
        thing. It is the same separator every menu in the app draws.
      */}
      <span
        aria-hidden="true"
        className="mx-1 h-4 w-px bg-slate-200 dark:bg-slate-700"
      />
      <IconButton
        label="Insert table"
        icon={Table}
        disabled={off}
        onClick={act((v) => insertBlock(v, TABLE_SKELETON))}
      />
      <IconButton
        label="Insert diagram"
        icon={Workflow}
        disabled={off}
        onClick={act((v) => insertBlock(v, MERMAID_SKELETON))}
      />
    </div>
  );
}
