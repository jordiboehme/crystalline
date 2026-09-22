/**
 * Both sides of one unshared change, drawn by CodeMirror's merge view: a
 * unified diff by default, two panes side by side from the large breakpoint,
 * markdown highlighting from the editor's own language, read-only, with
 * word-level marks inside a changed chunk. Nothing in it is a control:
 * accepting a chunk is not a verb here, discard is per file.
 *
 * Lazy-loaded through `DiffPaneLazy.tsx`: the merge package is a cost the
 * reading path never pays.
 */

import { syntaxHighlighting } from "@codemirror/language";
import { MergeView, unifiedMergeView } from "@codemirror/merge";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { useQuery } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useEffect, useRef, useSyncExternalStore } from "react";

import type { ChangeDetail } from "../api/admin";
import { fetchChange, localChangeKey } from "../api/admin";
import { problemDetail } from "../api/client";
import {
  documentLanguage,
  editorHighlight,
  editorTheme,
} from "../editor/setup";
import { useTheme } from "../theme/context";
import type { PaneLayout } from "./diffFace";
import { SPLIT_QUERY, kindWord, paneFace, sizeSentence } from "./diffFace";

/** The refusal face, the same one every other screen announces a problem in. */
const ALERT_CLASSES =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

/** The ceiling the conflict panes have: the pane scrolls, the dialog does not grow. */
const PANE_CLASSES =
  "max-h-[60vh] overflow-auto rounded border border-slate-200 dark:border-slate-800";

function wideNow(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia(SPLIT_QUERY).matches
  );
}

function subscribeWide(onChange: () => void): () => void {
  if (typeof window.matchMedia !== "function") {
    return () => undefined;
  }
  const query = window.matchMedia(SPLIT_QUERY);
  query.addEventListener("change", onChange);
  return () => {
    query.removeEventListener("change", onChange);
  };
}

/**
 * The pane's own faces for the merge classes, as tokens both schemes define in
 * index.css. The box around the view is what caps and scrolls it, so nothing
 * here sets a height.
 *
 * Every selector is written at the shape the package's own base theme uses,
 * side class and all, because that is what decides which of the two paints.
 * A plain `.cm-changedLine` lands at two classes against the base theme's
 * three (`&.cm-merge-a .cm-changedLine`) and loses every time - specificity
 * is read before precedence, and a base theme is mounted first, so raising
 * this layer would not have rescued it either. The word marks use the
 * `background` shorthand for the same reason: the base theme paints them with
 * a gradient through the shorthand, and a `background-color` longhand under
 * it is not the same property.
 */
const diffTheme = EditorView.theme({
  // The team's copy, and the chunks a unified view draws in its place.
  "&.cm-merge-a .cm-changedLine, .cm-deletedChunk": {
    backgroundColor: "var(--color-diff-removed)",
  },
  // This copy: the right-hand pane, and the whole buffer in a unified view,
  // which is the `b` side.
  "&.cm-merge-b .cm-changedLine, .cm-inlineChangedLine": {
    backgroundColor: "var(--color-diff-added)",
  },
  // Word level, inside a changed line, on either side and in either scheme.
  "&.cm-merge-a .cm-changedText, &.cm-merge-b .cm-changedText, .cm-deletedChunk .cm-deletedText":
    { background: "var(--color-diff-word)" },
});

function readOnly(dark: boolean) {
  return [
    EditorState.readOnly.of(true),
    EditorView.editable.of(false),
    // An engram's paragraph is one long line, so without this the pane
    // scrolls sideways for nearly every change it shows. A merge view aligns
    // its chunks with wrapping on.
    EditorView.lineWrapping,
    documentLanguage,
    syntaxHighlighting(editorHighlight),
    editorTheme(dark),
    diffTheme,
  ];
}

/** Mount the view for a face into `parent`; the returned function tears it down. */
function mountView(
  parent: HTMLElement,
  layout: PaneLayout,
  base: string,
  current: string,
  dark: boolean,
): () => void {
  if (layout === "split") {
    const view = new MergeView({
      parent,
      orientation: "a-b",
      highlightChanges: true,
      gutter: true,
      collapseUnchanged: { margin: 3, minSize: 4 },
      a: { doc: base, extensions: readOnly(dark) },
      b: { doc: current, extensions: readOnly(dark) },
    });
    return () => {
      view.destroy();
    };
  }
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: current,
      extensions: [
        unifiedMergeView({
          original: base,
          // There is nothing to accept: a chunk is not a decision here.
          mergeControls: false,
          highlightChanges: true,
          gutter: true,
          collapseUnchanged: { margin: 3, minSize: 4 },
        }),
        ...readOnly(dark),
      ],
    }),
  });
  return () => {
    view.destroy();
  };
}

/**
 * The line before the editor: what happened to the file, and how big it is.
 * A change whose sizes are both absent says only what happened.
 */
function caption(detail: ChangeDetail): string {
  const word = kindWord(detail.kind);
  const sizes = sizeSentence(
    detail.kind,
    detail.sizeBefore,
    detail.sizeAfter,
  ).replace(/^(Changed|Added|Deleted), /, "");
  return sizes === word ? word : `${word}, ${sizes}`;
}

export default function DiffPane({
  domain,
  path,
  headingId,
}: {
  domain: string;
  path: string;
  headingId: string;
}): ReactElement {
  const wide = useSyncExternalStore(subscribeWide, wideNow, () => false);
  const dark = useTheme().resolved === "dark";
  const host = useRef<HTMLDivElement>(null);
  // Filed under this domain's own prefix: reading a change is a plain GET with
  // no side effect, so a bulk invalidation costs a round trip and nothing
  // else. No retry - a refusal here is immediate and final.
  const detail = useQuery({
    queryKey: localChangeKey(domain, path),
    queryFn: () => fetchChange(domain, path),
    retry: false,
  });
  const face = detail.data ? paneFace(detail.data, wide) : null;
  // The view is rebuilt when any of these moves, so a re-read with different
  // text is a different diff rather than a stale one.
  const layout = face?.kind === "editor" ? face.layout : null;
  const baseText = face?.kind === "editor" ? face.base : null;
  const currentText = face?.kind === "editor" ? face.current : null;

  useEffect(() => {
    if (layout === null || host.current === null) {
      return;
    }
    return mountView(
      host.current,
      layout,
      baseText ?? "",
      currentText ?? "",
      dark,
    );
  }, [layout, baseText, currentText, dark]);

  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-2">
      {detail.error !== null && (
        <p role="alert" className={ALERT_CLASSES}>
          {problemDetail(detail.error)}
        </p>
      )}
      {/* Plain text before the editor, so a screen reader hears what it is
          about to read: the kind and the sizes. A sentence face says the same
          figures in its own words, so the caption stands only above a pane. */}
      {detail.data && face?.kind !== "sentence" && (
        <p className="text-caption text-slate-500 dark:text-slate-400">
          {caption(detail.data)}
        </p>
      )}
      {face?.kind === "sentence" && (
        <div className="flex flex-col gap-1 text-sm">
          <p>{face.text}</p>
          {face.hint !== null && (
            <code className="rounded bg-slate-100 px-1 font-mono text-xs dark:bg-slate-800">
              {face.hint.replace("<domain>", domain)}
            </code>
          )}
        </div>
      )}
      {face?.kind === "editor" && <div ref={host} className={PANE_CLASSES} />}
    </section>
  );
}
