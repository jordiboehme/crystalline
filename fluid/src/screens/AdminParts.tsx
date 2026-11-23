/**
 * Pieces the Administration pages share: the page heading with its one line,
 * and the field that shows a secret once.
 */

import type { ReactNode } from "react";
import { useEffect, useId, useRef } from "react";

import { BUTTON, CONTROL_HEIGHT, FIELD } from "../components/primitives";
import { copyText, useSaid } from "../components/useSaid";

/** A page heading with the one line under it that says what the page is for. */
export function PageHeader({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <header>
      <h1 className="text-display">{title}</h1>
      <p className="mt-1 text-sm text-muted">{children}</p>
    </header>
  );
}

/**
 * A secret the server hands over once: a token or an invite link, in a read
 * only field with a copy button and the sentence that it is not shown again.
 *
 * The value lives only in the caller's component state. Done, or leaving the
 * page, unmounts this and the value is gone. The field takes the focus when
 * it appears, so a keyboard user lands on what to copy.
 */
export function ShownOnce({
  label,
  copyLabel,
  value,
  note,
  onDone,
}: {
  /** The field's label, which names what this is for. */
  label: string;
  /** The copy button's accessible name, such as "Copy the token". */
  copyLabel: string;
  value: string;
  /** An extra line under the sentence, for how to use the value. */
  note?: ReactNode;
  onDone: () => void;
}) {
  const [said, say] = useSaid();
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => {
    field.current?.focus();
    field.current?.select();
  }, []);
  return (
    <div
      role="group"
      aria-label={label}
      className="flex flex-col gap-2 rounded border border-accent-600/40 bg-accent-50 p-3 dark:border-accent-400/40 dark:bg-accent-950/40"
    >
      <label className="flex flex-col gap-1 text-sm font-medium">
        {label}
        <input
          ref={field}
          readOnly
          value={value}
          onFocus={(event) => {
            event.currentTarget.select();
          }}
          className={`w-full font-mono ${FIELD}`}
        />
      </label>
      <p className="text-sm">Copy it now. It is not shown again.</p>
      {note}
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          aria-label={copyLabel}
          onClick={() => {
            copyText(value, say);
          }}
          className={`${CONTROL_HEIGHT} ${BUTTON.primary}`}
        >
          Copy
        </button>
        <button
          type="button"
          onClick={onDone}
          className={`${CONTROL_HEIGHT} ${BUTTON.secondary}`}
        >
          Done
        </button>
        <span role="status" className="text-caption text-ink-3">
          {said}
        </span>
      </div>
    </div>
  );
}

/** A notice at the top of a page: the server's sentence, or what just happened. */
export function NoticeLine({
  kind,
  text,
}: {
  kind: "problem" | "done";
  text: string;
}) {
  return (
    <p
      role={kind === "problem" ? "alert" : "status"}
      className={
        kind === "problem"
          ? "rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
          : "rounded border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300"
      }
    >
      {text}
    </p>
  );
}

/**
 * A form control with its label tied by id, so the control's name is the
 * label alone and never the text of the options inside it.
 */
export function Labelled({
  label,
  children,
}: {
  label: string;
  children: (id: string) => ReactNode;
}) {
  const id = useId();
  return (
    <div className="flex flex-col gap-1 text-sm">
      <label htmlFor={id}>{label}</label>
      {children(id)}
    </div>
  );
}
