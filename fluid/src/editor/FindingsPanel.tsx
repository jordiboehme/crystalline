/**
 * What verify would say, said before the save: the same rule families the
 * server runs on `crystalline verify`, run over the buffer on a pause in the
 * typing. Warnings advise and never block; hard errors name what a save
 * would corrupt, and the save button reads this panel's error count.
 */

import { EditorView } from "@codemirror/view";
import type { ReactElement } from "react";

import type { ValidateFinding, ValidateResponse } from "../api/model";

/**
 * Place the cursor on a one-based line and reveal it. Exported for reuse:
 * this screen's own `onJump` handler calls it, and so does the test that
 * checks it moves the selection without a live findings panel in the way.
 * The fast-refresh rule wants a component file to export only components;
 * splitting one small pure helper into a second file for that alone would
 * scatter this panel's whole exported surface for no real benefit.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function jumpToLine(view: EditorView, line: number): void {
  // A server line is counted against the file's own text, which can diverge
  // from the buffer's line count for a mixed-ending document; clamped rather
  // than trusted, so a stale or out-of-range finding still lands somewhere
  // sane instead of throwing.
  const clamped = Math.max(1, Math.min(line, view.state.doc.lines));
  const at = view.state.doc.line(clamped).from;
  view.dispatch({
    selection: { anchor: at },
    effects: EditorView.scrollIntoView(at, { y: "center" }),
  });
  view.focus();
}

export interface FindingsPanelProps {
  /** null = nothing run yet. */
  report: ValidateResponse | null;
  pending: boolean;
  onJump: (line: number) => void;
}

const SEVERITY_CLASSES: Record<string, string> = {
  error: "bg-red-50 text-red-800 dark:bg-red-950 dark:text-red-200",
  warning: "bg-amber-50 text-amber-900 dark:bg-amber-950 dark:text-amber-100",
  info: "bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-300",
};

export function FindingsPanel({
  report,
  pending,
  onJump,
}: FindingsPanelProps): ReactElement {
  return (
    <section aria-label="Validation findings" className="flex flex-col gap-2">
      <h2 className="text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">
        Findings
      </h2>
      {report === null || (report.findings.length === 0 && pending) ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">Checking</p>
      ) : report.findings.length === 0 ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Nothing to fix. The format and temporal rules are satisfied; the wider
          link and schema rules run on `crystalline verify` over the whole
          domain.
        </p>
      ) : (
        <ul className="flex flex-col gap-2 text-sm">
          {report.findings.map((finding: ValidateFinding, index) => (
            <li
              key={`${finding.rule}-${String(finding.line)}-${String(index)}`}
              className="flex flex-col gap-1"
            >
              <span className="flex flex-wrap items-baseline gap-2">
                <span
                  className={`rounded px-1.5 py-0.5 font-mono text-xs ${SEVERITY_CLASSES[finding.severity] ?? SEVERITY_CLASSES.info ?? ""}`}
                >
                  {finding.rule}
                </span>
                <span>{finding.message}</span>
              </span>
              {finding.fix != null && (
                <span className="text-xs text-slate-500 dark:text-slate-400">
                  {finding.fix}
                </span>
              )}
              {finding.line != null && (
                <button
                  type="button"
                  className="self-start text-xs text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
                  onClick={() => {
                    onJump(finding.line ?? 1);
                  }}
                >
                  Go to line {finding.line}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
