/**
 * The "Aa" menu: what the caret's line is, as one radio group - a title, a
 * heading or body text. It replaces the old Heading menu. The sticky format
 * bar has it in every mode, Raw included, and the floating "Quick format"
 * bar has it on a text selection, so the two bars say it with one set of
 * names.
 *
 * The menu owns the whole gesture: the write (`setHeadingLevel`), and the
 * question a Reflection page asks before a section heading stops being one,
 * with its dialog. Both bars mount this one component, so the menu, its write
 * and its dialog cannot drift apart. What differs between the bars comes in
 * as props: whose turn it is (`allow`), where the question comes from
 * (`question`), and how the trigger and the layers sit in the bar.
 */

import type { EditorView } from "@codemirror/view";
import { Check } from "lucide-react";
import { AlertDialog, DropdownMenu } from "radix-ui";
import { useRef, useState } from "react";
import type { ButtonHTMLAttributes, ReactElement, ReactNode } from "react";

import { ITEM_CLASSES, MENU_CLASSES } from "../components/menu";
import { BUTTON, Tooltip } from "../components/primitives";
import { stepOffIdLines } from "./hiddenIdLines";
import { TRIGGER } from "./InsertMenu";
import { judgeBeforeCommand } from "./marks/commandJudge";
import { setHeadingLevel } from "./toolbar";

/** The number of `#` a style writes. */
export type StyleLevel = 0 | 1 | 2 | 3 | 4;

/**
 * The document's question before a heading change on line `line` (1-based)
 * that leaves it at `level`: the sentence to confirm, or null to go ahead.
 */
export type HeadingQuestion = (
  state: EditorView["state"],
  line: number,
  level: number,
) => string | null;

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

/** Attributes a bar puts on the menu's layers, such as its own mark. */
type LayerAttributes = Record<`data-${string}`, string>;

/**
 * The one question a heading change can ask, in the shape of the other
 * dialogs in the editor (`ConfirmLeaveDialog`): the sentence the adapter
 * answered, the change as the primary answer, Cancel quiet. Escape and the
 * scrim mean Cancel.
 */
function ConfirmHeadingDialog({
  sentence,
  layer,
  onConfirm,
  onCancel,
  onClosed,
}: {
  sentence: string;
  layer: LayerAttributes;
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
          {...layer}
          // React hands a portal's events to the tree it was rendered in:
          // the bar's own mousedown guard is for its verbs, not this dialog.
          onMouseDown={(event) => {
            event.stopPropagation();
          }}
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

export function TextStyleMenu({
  view,
  disabled,
  allow = () => true,
  question,
  trigger = {},
  inline = false,
  layer = {},
}: {
  /** The view the style is written to. */
  view: EditorView | null;
  disabled: boolean;
  /** Whether it is the view's turn when a style is picked (the bar's rule). */
  allow?: (view: EditorView) => boolean;
  /** The document's question before a heading change, if it asks one. */
  question: HeadingQuestion | null;
  /** More attributes for the trigger, such as a roving tab stop's. */
  trigger?: ButtonHTMLAttributes<HTMLButtonElement> & LayerAttributes;
  /**
   * The menu drawn inside the bar instead of in a portal, so the focus
   * moving into it never reads as the focus leaving the bar.
   */
  inline?: boolean;
  /** Attributes for the menu and the dialog, such as the bar's own mark. */
  layer?: LayerAttributes;
}): ReactElement {
  // Read as the menu opens: the caret moves without the bar re-rendering.
  const [current, setCurrent] = useState("");
  /** The heading change waiting for the person's answer, if any. */
  const [asking, setAsking] = useState<{
    level: StyleLevel;
    sentence: string;
  } | null>(null);
  /**
   * Set as a pick asks first, so the menu's close hands focus to the dialog
   * rather than back to the buffer. A ref, because the menu closes before
   * the state above has rendered.
   */
  const askingNow = useRef(false);

  /** The question a heading change on the caret's line asks, or null. */
  const questionFor = (v: EditorView, level: StyleLevel): string | null => {
    const line = v.state.doc.lineAt(v.state.selection.main.head).number;
    return question?.(v.state, line, level) ?? null;
  };
  const pick = (level: StyleLevel) => {
    if (view === null || !allow(view)) {
      return;
    }
    judgeBeforeCommand(view);
    // The line the change is made on, as `setHeadingLevel` will find it: off
    // a hidden id line first, so the question is about that same line.
    stepOffIdLines(view);
    const sentence = questionFor(view, level);
    if (sentence === null) {
      setHeadingLevel(view, level);
      return;
    }
    askingNow.current = true;
    setAsking({ level, sentence });
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

  const content = (
    <DropdownMenu.Content
      align="start"
      sideOffset={6}
      className={MENU_CLASSES}
      {...layer}
      // Radix hands focus back to the trigger when the menu closes, which
      // would leave the caret out of the buffer right after a command put
      // it back there. The buffer is where an author wants to be either
      // way, so the menu always returns them - unless a dialog asks.
      onCloseAutoFocus={(event) => {
        event.preventDefault();
        if (!askingNow.current) {
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
              pick(level);
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
  );
  const placed: ReactNode = inline ? (
    content
  ) : (
    <DropdownMenu.Portal>{content}</DropdownMenu.Portal>
  );

  return (
    <>
      <DropdownMenu.Root
        onOpenChange={(open) => {
          if (open) {
            setCurrent(currentStyle(view));
          }
        }}
      >
        <Tooltip label="Text style">
          <DropdownMenu.Trigger asChild>
            <button
              type="button"
              aria-label="Text style"
              className={`${TRIGGER} font-semibold`}
              disabled={disabled}
              {...trigger}
            >
              Aa
            </button>
          </DropdownMenu.Trigger>
        </Tooltip>
        {placed}
      </DropdownMenu.Root>
      {asking !== null && (
        <ConfirmHeadingDialog
          sentence={asking.sentence}
          layer={layer}
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
