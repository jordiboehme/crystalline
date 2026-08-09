/**
 * The structured view over the frontmatter block. Reading derives from the
 * live buffer text on every render; writing dispatches the one-line edit the
 * field helpers compute. The buffer stays the single truth: the form never
 * holds its own copy of a value, so a hand edit in the text is on the form a
 * render later, and a form edit is one ordinary undoable transaction.
 *
 * Temporal semantics: an empty date input IS the unbounded state. Clearing
 * removes the key; nothing here ever writes a placeholder date.
 *
 * The recommended `type` and `status` values are the app's one list, offered
 * through a datalist beside a plain text field. Anything can be typed and
 * nothing here treats an unlisted value as wrong - the fields are free form
 * by design, and a select would quietly claim otherwise.
 */

import type { Text } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { ReactElement, ReactNode } from "react";
import { useState } from "react";

import type { Vocabulary } from "../api/vocabulary";
import { FIELD_CLASSES } from "../components/FilterControls";
import { SUGGESTED_STATUSES, SUGGESTED_TYPES } from "../filters";
import type { FieldEdit } from "./frontmatterFields";
import {
  hasFrontmatterBlock,
  readScalar,
  readTagList,
  writeScalar,
  writeTagList,
} from "./frontmatterFields";

/**
 * Guidance, never enforcement. The same lists the filtering screens offer, so
 * the words this app recommends for a field are one list rather than two that
 * drift apart.
 */
export const RECOMMENDED_STATUSES: readonly string[] = SUGGESTED_STATUSES;
export const RECOMMENDED_TYPES: readonly string[] = SUGGESTED_TYPES;

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

const NOTE_CLASSES = "text-xs text-slate-500 dark:text-slate-400";

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1 text-sm">
      <span className="text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">
        {label}
      </span>
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

  const apply = (edit: FieldEdit | null) => {
    if (!edit || !view) {
      return;
    }
    const separator = view.state.lineBreak;
    view.dispatch({
      changes: {
        from: positionOf(view.state.doc, doc, edit.from, separator),
        to: positionOf(view.state.doc, doc, edit.to, separator),
        // The helpers write the separator they found in the text; the state
        // is the authority on what this buffer's break actually is.
        insert: edit.insert.split(/\r\n|\n/).join(separator),
      },
    });
  };
  const scalar = (key: string) => (value: string) => {
    apply(writeScalar(doc, key, value.trim() === "" ? null : value.trim()));
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
      <Row label="Type">
        {/*
          Uncontrolled with a remounting key: what is on screen while the
          field has focus is what is being typed, and a hand edit in the text
          changes the key and brings the new value in. The same
          state-follows-the-source pattern the filter fields use.
        */}
        <input
          className={`w-full ${FIELD_CLASSES}`}
          list="fm-types"
          key={`type:${type}`}
          defaultValue={type}
          onBlur={(event) => {
            scalar("type")(event.target.value);
          }}
        />
        <datalist id="fm-types">
          {RECOMMENDED_TYPES.map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
      </Row>
      <Row label="Status">
        <input
          className={`w-full ${FIELD_CLASSES}`}
          list="fm-statuses"
          key={`status:${status}`}
          defaultValue={status}
          onBlur={(event) => {
            scalar("status")(event.target.value);
          }}
        />
        <datalist id="fm-statuses">
          {RECOMMENDED_STATUSES.map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
      </Row>
      {/*
        Not a label element: the row holds a button per tag as well as the
        field, and a label wrapping several controls names none of them
        clearly. The field carries its own name instead.
      */}
      <div className="flex flex-col gap-1 text-sm">
        <span className="text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">
          Tags
        </span>
        {tags.length > 0 && (
          <span className="flex flex-wrap gap-1">
            {tags.map((tag) => (
              <button
                key={tag}
                type="button"
                aria-label={`Remove tag ${tag}`}
                className="rounded bg-slate-100 px-1.5 py-0.5 text-xs hover:line-through focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:bg-slate-800"
                onClick={() => {
                  apply(
                    writeTagList(
                      doc,
                      tags.filter((existing) => existing !== tag),
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
              apply(writeTagList(doc, [...tags, draftTag.trim()]));
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
        doc={doc}
        onEdit={apply}
      />
      <DateRow label="Valid to" keyName="valid_to" doc={doc} onEdit={apply} />
      <p className={NOTE_CLASSES}>
        An empty date means unbounded validity. Recommended types and statuses
        are suggestions; any value is allowed.
      </p>
    </section>
  );
}

/**
 * One temporal bound. Empty is not a missing answer, it is the answer: the
 * knowledge has always been valid, or is valid forever. Clearing the field
 * removes the key rather than writing anything in its place.
 */
function DateRow({
  label,
  keyName,
  doc,
  onEdit,
}: {
  label: string;
  keyName: string;
  doc: string;
  onEdit: (edit: FieldEdit | null) => void;
}) {
  const value = readScalar(doc, keyName) ?? "";
  return (
    <div className="flex items-end gap-2">
      <Row label={label}>
        <input
          type="date"
          className={`w-full ${FIELD_CLASSES}`}
          value={value}
          onChange={(event) => {
            onEdit(
              writeScalar(
                doc,
                keyName,
                event.target.value === "" ? null : event.target.value,
              ),
            );
          }}
        />
      </Row>
      {value !== "" && (
        <button
          type="button"
          aria-label={`Clear ${label.toLowerCase()}`}
          className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
          onClick={() => {
            onEdit(writeScalar(doc, keyName, null));
          }}
        >
          Clear
        </button>
      )}
    </div>
  );
}
