/**
 * The "Aa" menu of the format bar: what the caret's line is, as one radio
 * group - a title, a heading or body text. It replaces the old Heading menu
 * and is in the bar in every mode, Raw included.
 *
 * The menu only chooses. The bar owns the write (`setHeadingLevel`) and the
 * question a Reflection page asks before a section heading stops being one,
 * so the menu and its dialog cannot drift apart.
 */

import type { EditorView } from "@codemirror/view";
import { Check } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { useState } from "react";
import type { ReactElement } from "react";

import { ITEM_CLASSES, MENU_CLASSES } from "../components/menu";
import { TRIGGER } from "./InsertMenu";

/** The number of `#` a style writes. */
export type StyleLevel = 0 | 1 | 2 | 3 | 4;

/** The styles in the order they read, each with the `#` count it writes. */
const STYLES: { label: string; level: StyleLevel }[] = [
  { label: "Title", level: 1 },
  { label: "Heading 1", level: 2 },
  { label: "Heading 2", level: 3 },
  { label: "Heading 3", level: 4 },
  { label: "Body", level: 0 },
];

const HEADING = /^(#{1,6})[ \t]+/;

/** The style the main selection's line wears; deeper headings match none. */
function currentStyle(view: EditorView | null): string {
  if (view === null) {
    return "";
  }
  const { state } = view;
  const text = state.doc.lineAt(state.selection.main.head).text;
  const worn = HEADING.exec(text)?.[1]?.length ?? 0;
  return STYLES.find((style) => style.level === worn)?.label ?? "";
}

export function TextStyleMenu({
  view,
  disabled,
  onPick,
  keepsFocus,
}: {
  view: EditorView | null;
  disabled: boolean;
  /** The bar's pick: the write, or the question before it. */
  onPick: (level: StyleLevel) => void;
  /** True while a dialog is about to take the focus the menu would return. */
  keepsFocus: () => boolean;
}): ReactElement {
  // Read as the menu opens: the caret moves without the bar re-rendering.
  const [current, setCurrent] = useState("");
  return (
    <DropdownMenu.Root
      onOpenChange={(open) => {
        if (open) {
          setCurrent(currentStyle(view));
        }
      }}
    >
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label="Text style"
          className={`${TRIGGER} font-semibold`}
          disabled={disabled}
        >
          Aa
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={6}
          className={MENU_CLASSES}
          // Radix hands focus back to the trigger when the menu closes, which
          // would leave the caret out of the buffer right after a command put
          // it back there. The buffer is where an author wants to be either
          // way, so this bar always returns them - unless a dialog asks.
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            if (!keepsFocus()) {
              view?.focus();
            }
          }}
        >
          <DropdownMenu.RadioGroup value={current}>
            {STYLES.map(({ label, level }) => (
              <DropdownMenu.RadioItem
                key={label}
                value={label}
                className={ITEM_CLASSES}
                onSelect={() => {
                  onPick(level);
                }}
              >
                {label}
                <DropdownMenu.ItemIndicator className="ml-auto">
                  <Check aria-hidden size={14} />
                </DropdownMenu.ItemIndicator>
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
