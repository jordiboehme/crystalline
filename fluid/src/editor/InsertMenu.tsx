/**
 * The format bar's one Insert menu: callouts, diagrams, blocks and inline
 * elements, as words, read from the same registry as the `/` menu
 * (`inserts.ts`).
 *
 * Two entries of the registry are not listed here because the bar already
 * has them as controls of their own, one place per control: the table (the
 * size picker beside this menu) and the task list (its own button).
 *
 * Callout and Diagram are submenus. The diagram submenu keeps the three
 * starter groups with their headings, each heading named as its group's
 * label, and scrolls when the window is shorter than its nineteen rows.
 */

import type { EditorView } from "@codemirror/view";
import { ChevronRight, Plus } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import type { ReactElement } from "react";
import { useId } from "react";

import {
  ITEM_CLASSES,
  MENU_CLASSES,
  MENU_LABEL,
  MENU_SEPARATOR,
} from "../components/menu";
import { FOCUS_RING } from "../components/primitives";
import type { InsertAction } from "./inserts";
import { INSERT_ACTIONS } from "./inserts";
import { judgeBeforeCommand } from "./marks/commandJudge";

/** The registry entries the bar shows as controls of their own. */
const OWN_CONTROLS = new Set(["table", "task"]);

const LISTED = INSERT_ACTIONS.filter((action) => !OWN_CONTROLS.has(action.id));

/** The diagram submenu: the menu face, with a ceiling it scrolls under. */
const TALL_SURFACE = `${MENU_CLASSES} max-h-[min(70vh,var(--radix-dropdown-menu-content-available-height))] overflow-y-auto`;

/** The toolbar button: the word and a plus, the bar's own quiet face. */
export const TRIGGER = `inline-flex h-8 shrink-0 items-center gap-1.5 rounded px-2 text-[13px] text-slate-600 hover:bg-slate-100 disabled:opacity-50 data-[state=open]:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800 dark:data-[state=open]:bg-slate-800 ${FOCUS_RING}`;

function of(group: InsertAction["group"]): InsertAction[] {
  return LISTED.filter((action) => action.group === group);
}

export function InsertMenu({
  view,
  allow,
}: {
  /** The buffer the entries edit; null turns the button off. */
  view: EditorView | null;
  /**
   * Asked when an entry runs: false refuses it (the sticky bar's guard
   * while main's selection is parked behind a table cell).
   */
  allow?: (view: EditorView) => boolean;
}): ReactElement {
  const stem = useId();
  const run = (action: InsertAction) => () => {
    if (view && (allow?.(view) ?? true)) {
      judgeBeforeCommand(view);
      action.run(view);
    }
  };
  const item = (action: InsertAction) => (
    <DropdownMenu.Item
      key={action.id}
      className={ITEM_CLASSES}
      onSelect={run(action)}
    >
      {action.label}
    </DropdownMenu.Item>
  );
  /** A labelled section, its label named as the group's accessible name. */
  const section = (label: string, actions: InsertAction[], key: string) => (
    <DropdownMenu.Group key={key} aria-labelledby={`${stem}-${key}`}>
      <DropdownMenu.Label id={`${stem}-${key}`} className={MENU_LABEL}>
        {label}
      </DropdownMenu.Label>
      {actions.map(item)}
    </DropdownMenu.Group>
  );
  const sub = (label: string, children: ReactElement[], tall = false) => (
    <DropdownMenu.Sub>
      <DropdownMenu.SubTrigger className={ITEM_CLASSES}>
        {label}
        <ChevronRight
          aria-hidden="true"
          size={14}
          strokeWidth={1.75}
          className="ml-auto text-muted"
        />
      </DropdownMenu.SubTrigger>
      <DropdownMenu.Portal>
        <DropdownMenu.SubContent
          aria-label={label}
          sideOffset={6}
          className={tall ? TALL_SURFACE : MENU_CLASSES}
        >
          {children}
        </DropdownMenu.SubContent>
      </DropdownMenu.Portal>
    </DropdownMenu.Sub>
  );
  const diagrams = of("Diagram");
  const subgroups = [...new Set(diagrams.map((action) => action.subgroup))];
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button type="button" className={TRIGGER} disabled={view === null}>
          <Plus aria-hidden="true" size={16} strokeWidth={1.75} />
          Insert
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={6}
          className={MENU_CLASSES}
          // Radix hands focus back to the trigger when the menu closes; the
          // bar always returns an author to the buffer instead, having
          // inserted something or having pressed Escape.
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            // Not over a field the insert opened in the text, as a starter's does.
            if (!view?.dom.contains(document.activeElement)) {
              view?.focus();
            }
          }}
        >
          {sub("Callout", of("Callout").map(item))}
          {sub(
            "Diagram",
            subgroups.map((subgroup, index) =>
              section(
                subgroup ?? "",
                diagrams.filter((action) => action.subgroup === subgroup),
                `diagram-${String(index)}`,
              ),
            ),
            true,
          )}
          <DropdownMenu.Separator className={MENU_SEPARATOR} />
          {section("Blocks", of("Blocks"), "blocks")}
          {section("Inline", of("Inline"), "inline")}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
