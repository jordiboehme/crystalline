/**
 * The structured view over the frontmatter block. Reading derives from the
 * live buffer text on every render; writing dispatches the one-line edit the
 * field helpers compute. The buffer stays the single truth: the form never
 * holds its own copy of a value, so a hand edit in the text is on the form a
 * render later, and a form edit is one ordinary undoable transaction.
 *
 * Temporal semantics: an absent bound is an answer rather than a gap, so it is
 * drawn as one - "Always" for the lower bound, "Forever" for the upper - and
 * one click swaps that state for a picker with the keyboard already in it.
 * Clearing the picker is the one thing that removes a key, and nothing here
 * ever writes a placeholder date.
 *
 * The recommended `type` and `status` values are the app's one list, offered
 * through the suggesting input: the words are on screen with a line each on
 * what they are for, so nobody has to have memorized the set to write one
 * down. Anything can be typed and nothing here treats an unlisted value as
 * wrong - the fields are free form by design, and a select would quietly
 * claim otherwise.
 */

import type { Text } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { X } from "lucide-react";
import type { ReactElement, ReactNode } from "react";
import { useEffect, useId, useRef, useState } from "react";

import type { Vocabulary } from "../api/vocabulary";
import { FIELD_CLASSES } from "../components/FilterControls";
import { BUTTON, FOCUS_RING, IconButton } from "../components/primitives";
import { SuggestInput } from "../components/SuggestInput";
import { STATUS_SUGGESTIONS, TYPE_SUGGESTIONS } from "../suggestions";
import type { FieldEdit } from "./frontmatterFields";
import {
  hasFrontmatterBlock,
  readScalar,
  readTagList,
  writeScalar,
  writeTagList,
} from "./frontmatterFields";
import { docText } from "./setup";

export interface FrontmatterFormProps {
  /** The live buffer text, which is where every value is read from. */
  doc: string;
  /** Where edits dispatch. Null until the view has mounted. */
  view: EditorView | null;
  /** The domain's words, for the tag suggestions. */
  vocabulary: Vocabulary | null;
}

/**
 * A string offset in `text` as a position in the document.
 *
 * The two do not agree by themselves: `Text` counts a line break as one
 * position whatever the separator is, while a CRLF buffer's own text spends
 * two characters on it. Counting lines and columns and asking the document
 * where that line starts is the translation, and it is the reason the helpers
 * are pure string mathematics and this file owns the dispatch.
 */
function positionOf(
  doc: Text,
  text: string,
  offset: number,
  separator: string,
): number {
  const parts = text.slice(0, offset).split(separator);
  const line = doc.line(Math.min(parts.length, doc.lines));
  const column = parts.at(-1)?.length ?? 0;
  return Math.min(line.from + column, line.to);
}

const NOTE_CLASSES = "text-caption text-slate-500 dark:text-slate-400";

/**
 * The rail's field labels: sentence case at the caption step, never shouted.
 * The words themselves are the accessible names of the controls they sit on,
 * so they are the one thing here that may not drift.
 */
const LABEL_CLASSES =
  "text-caption font-medium text-slate-600 dark:text-slate-300";

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1 text-sm">
      <span className={LABEL_CLASSES}>{label}</span>
      {children}
    </label>
  );
}

export function FrontmatterForm({
  doc,
  view,
  vocabulary,
}: FrontmatterFormProps): ReactElement {
  const [draftTag, setDraftTag] = useState("");
  const typeField = useId();
  const statusField = useId();
  const guidance = useId();

  const apply = (compute: (base: string) => FieldEdit | null) => {
    if (!view) {
      return;
    }
    // The live text, read here and handed to the caller, rather than the `doc`
    // prop. The prop is React state and can be one transaction behind the
    // document inside a nested event turn: the toolbar calls `view.focus()`,
    // that blurs a rail field with an uncommitted value, and the commit runs
    // before React has flushed the buffer the toolbar's own edit produced. An
    // edit computed on the old text and dispatched against the new document
    // resolves to shifted lines, which is how an Insert table could land
    // inside the frontmatter block or eat its closing fence.
    //
    // One reader, so the two can no longer disagree: whatever `compute`
    // measures its offsets against is exactly what `positionOf` translates
    // them through. Every dispatch in this file goes through here.
    const base = docText(view.state);
    const edit = compute(base);
    if (!edit) {
      return;
    }
    const separator = view.state.lineBreak;
    view.dispatch({
      changes: {
        from: positionOf(view.state.doc, base, edit.from, separator),
        to: positionOf(view.state.doc, base, edit.to, separator),
        // The helpers write the separator they found in the text; the state
        // is the authority on what this buffer's break actually is.
        insert: edit.insert.split(/\r\n|\n/).join(separator),
      },
    });
  };
  const scalar = (key: string) => (value: string) => {
    apply((base) =>
      writeScalar(base, key, value.trim() === "" ? null : value.trim()),
    );
  };

  const tags = readTagList(doc);
  const type = readScalar(doc, "type") ?? "";
  const status = readScalar(doc, "status") ?? "";
  const salience = readScalar(doc, "salience") ?? "";

  if (!hasFrontmatterBlock(doc)) {
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        This document has no frontmatter block yet, so there is nothing here to
        assist with. Add one at the top of the text: a save requires it.
      </p>
    );
  }

  return (
    <section aria-label="Frontmatter form" className="flex flex-col gap-3">
      {/*
        Not a `Row`: a `label` element wrapping the control would also wrap the
        list it opens, and a click on a suggestion would be a click on the
        label. The name is tied on with `htmlFor` instead, which is the same
        name to a reader and to a screen reader.

        The buffer is still the only copy of the value: the field is handed
        what the document currently says and reports back what settled, so a
        hand edit in the text arrives here a render later.
      */}
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={typeField} className={LABEL_CLASSES}>
          Type
        </label>
        <SuggestInput
          id={typeField}
          label="Type"
          className={`w-full ${FIELD_CLASSES}`}
          value={type}
          suggestions={TYPE_SUGGESTIONS}
          onCommit={scalar("type")}
          describedBy={guidance}
        />
      </div>
      <div className="flex flex-col gap-1 text-sm">
        <label htmlFor={statusField} className={LABEL_CLASSES}>
          Status
        </label>
        <SuggestInput
          id={statusField}
          label="Status"
          className={`w-full ${FIELD_CLASSES}`}
          value={status}
          suggestions={STATUS_SUGGESTIONS}
          onCommit={scalar("status")}
          describedBy={guidance}
        />
      </div>
      {/*
        Beside the two fields it is about, rather than at the foot of the rail
        where it used to sit half of a note about dates. It survives the
        suggesting input rather than being subsumed by it: a list of words is
        exactly what a closed set looks like, so the one thing a popover cannot
        say on its own is that anything else is allowed too. Both fields point
        at it, so it is read out with either name.
      */}
      <p id={guidance} className={NOTE_CLASSES}>
        Recommended types and statuses are suggestions; any value is allowed.
      </p>
      {/*
        Not a label element: the row holds a button per tag as well as the
        field, and a label wrapping several controls names none of them
        clearly. The field carries its own name instead.
      */}
      <div className="flex flex-col gap-1 text-sm">
        <span className={LABEL_CLASSES}>Tags</span>
        {tags.length > 0 && (
          <span className="flex flex-wrap gap-1">
            {tags.map((tag) => (
              <button
                key={tag}
                type="button"
                aria-label={`Remove tag ${tag}`}
                className={`rounded bg-slate-100 px-1.5 py-0.5 text-caption hover:line-through dark:bg-slate-800 ${FOCUS_RING}`}
                onClick={() => {
                  // The list is re-read from the base too, not closed over
                  // from the render: the tag to drop is the fact this button
                  // carries, and everything else about the block is whatever
                  // the document says at the moment of the dispatch.
                  apply((base) =>
                    writeTagList(
                      base,
                      readTagList(base).filter((existing) => existing !== tag),
                    ),
                  );
                }}
              >
                #{tag}
              </button>
            ))}
          </span>
        )}
        <input
          className={`w-full ${FIELD_CLASSES}`}
          aria-label="Add tag"
          list="fm-tags"
          value={draftTag}
          onChange={(event) => {
            setDraftTag(event.target.value);
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && draftTag.trim() !== "") {
              event.preventDefault();
              apply((base) =>
                writeTagList(base, [...readTagList(base), draftTag.trim()]),
              );
              setDraftTag("");
            }
          }}
          placeholder="add a tag, then Enter"
        />
        <datalist id="fm-tags">
          {(vocabulary?.tags ?? []).map((tag) => (
            <option key={tag.name} value={tag.name} />
          ))}
        </datalist>
      </div>
      <Row label="Salience">
        <input
          type="number"
          step="0.1"
          className={`w-full ${FIELD_CLASSES}`}
          key={`salience:${salience}`}
          defaultValue={salience}
          onBlur={(event) => {
            scalar("salience")(event.target.value);
          }}
        />
      </Row>
      <DateRow
        label="Valid from"
        keyName="valid_from"
        unbounded="Always"
        doc={doc}
        onEdit={apply}
      />
      <DateRow
        label="Valid to"
        keyName="valid_to"
        unbounded="Forever"
        doc={doc}
        onEdit={apply}
      />
    </section>
  );
}

/**
 * One temporal bound, in whichever of its two states it is in.
 *
 * No date is not a missing answer, it is the answer - the knowledge has always
 * been valid, or is valid forever - so it is drawn as a named state rather than
 * as an empty field somebody has to know how to read. That state is a button:
 * pressing it puts a picker there instead, with the keyboard already in it, and
 * writes nothing until a date is actually picked. The way back out is a clear
 * control that is there from the moment the picker is, so an author who opened
 * one by accident is one click from where they were, as often as they like.
 *
 * Only a complete date is written, and the clear control is the one thing that
 * removes the key. A date control reports every partly entered date as the
 * empty string, so a field that treated empty as "remove" would delete the
 * line the moment somebody started retyping a date and re-add it at the
 * bottom of the block when they finished - line churn in the one module whose
 * whole purpose is not making any, and a savable intermediate state where the
 * engram has silently lost its bound. Removal stays an act somebody performs
 * rather than a state their typing passes through.
 *
 * The buffer is still the only copy of the value: `picking` says nothing about
 * the document, only that a picker is on screen. A hand edit that writes the
 * key shows the date, whichever state the row is in. A hand edit that REMOVES
 * the key puts the named state back only while no picker is open - an open
 * picker outlives it and stands there empty, because `picking` is what somebody
 * asked for and the text going quiet is not them changing their mind. The clear
 * control beside it is the way back, and it is there the whole time.
 */
function DateRow({
  label,
  keyName,
  unbounded,
  doc,
  onEdit,
}: {
  label: string;
  keyName: string;
  /** What this bound's absence is called: "Always" low, "Forever" high. */
  unbounded: string;
  /** What the row DISPLAYS, which is a render and may be a render behind. */
  doc: string;
  /**
   * What the row WRITES: a thunk handed the live text at dispatch time. The
   * two are deliberately different sources - see `apply`.
   */
  onEdit: (compute: (base: string) => FieldEdit | null) => void;
}) {
  const value = readScalar(doc, keyName) ?? "";
  const [picking, setPicking] = useState(false);
  const field = useRef<HTMLInputElement>(null);
  // The swap hands the keyboard over: the control that was pressed is gone, so
  // focus would otherwise fall to the body. Keyed on `picking` alone, so it
  // fires on the swap and never steals focus on an ordinary render - a form
  // that autofocused its dates would take the caret out of the buffer every
  // time the editor opened on an engram that has one.
  useEffect(() => {
    if (picking) {
      field.current?.focus();
    }
  }, [picking]);

  if (value === "" && !picking) {
    return (
      // Not a `label` element: it names a button, and a button takes its name
      // from what it says. The word is the state, and the accessible name says
      // which bound is in it.
      <div className="flex flex-col gap-1 text-sm">
        <span className={LABEL_CLASSES}>{label}</span>
        <button
          type="button"
          aria-label={`${label}: ${unbounded}`}
          // The field's own box, quietly: pressing it puts a picker in exactly
          // this space, so nothing moves when it does. The dashed border is
          // what makes the box visible while it is empty - dashed rather than
          // the picker's solid rule, because an absent bound is an answer and
          // a box drawn like a filled field would read as a gap in one. It
          // shifts nothing: the border is inside the same `h-8`, and the 1px
          // it takes is the 1px the picker's own border takes, so the word
          // sits exactly where the date will.
          //
          // RULING (Checkpoint D): this border is a redundant affordance - the
          // state is already carried by the word inside it - so its 1.48:1
          // light / 1.72:1 dark against the rail is accepted under the same
          // reasoning as the divider precedent rather than the 3:1 floor.
          className={`${BUTTON.ghost} inline-flex h-8 w-full items-center justify-start border border-dashed border-slate-300 dark:border-slate-700`}
          onClick={() => {
            setPicking(true);
          }}
        >
          {unbounded}
        </button>
      </div>
    );
  }

  return (
    <div className="flex items-end gap-2">
      <Row label={label}>
        <input
          ref={field}
          type="date"
          className={`w-full ${FIELD_CLASSES}`}
          value={value}
          onChange={(event) => {
            if (event.target.value !== "") {
              const picked = event.target.value;
              onEdit((base) => writeScalar(base, keyName, picked));
            }
          }}
        />
      </Row>
      <IconButton
        label={`Clear to ${unbounded.toLowerCase()}`}
        icon={X}
        onClick={() => {
          // Both halves of going back: the key goes, and so does the empty
          // picker that was opened without one ever being written.
          setPicking(false);
          onEdit((base) => writeScalar(base, keyName, null));
        }}
      />
    </div>
  );
}
