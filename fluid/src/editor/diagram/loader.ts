/**
 * The eager stub for the lazy diagram chunk.
 *
 * The shape of the grid's loader (`grid/loader.ts`). Nothing here imports
 * `./index` statically: the chunk is asked for with a dynamic import, the
 * first time a view holds a top-level, closed mermaid fence of an editable
 * kind (or a test calls `prefetchDiagrams`). After the first look a view looks
 * again only where an edit or a parse step touched a fenced block, so a page
 * with no diagram does not walk its whole tree on each key. One compartment and one load
 * for the whole app - `previewConfig` runs again on every Raw flip, and a
 * compartment made per call would forget what the chunk brought.
 *
 * Until the chunk is in, nothing changes in the editor: every mermaid fence
 * keeps its read-only preview. A failed import (a network blip, or a
 * redeploy that changed the chunk's hash) is tried again after 5 seconds,
 * then after 30. After the third failure the loader gives up quietly: the
 * fences stay text with their preview, which needs no notice. A failed
 * import is no verdict about any fence and is not remembered: the next
 * request (an edit that leaves a fence in the document, a new view, a
 * prefetch) tries again from the start.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension } from "@codemirror/state";
import { Compartment } from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import { ViewPlugin } from "@codemirror/view";

import { parseAdvanced } from "../parseProgress";
import { diagramKindOf, EDITABLE_KINDS } from "./draws";
import { mermaidFences } from "./fence";

interface DiagramChunk {
  diagramExtension: () => Extension;
}

export interface DiagramLayer {
  /** The extension to place in a state: the compartment and its watcher. */
  extension: () => Extension;
  compartment: Compartment;
  /** Starts the load; resolves when it settled, whichever way. */
  prefetch: () => Promise<void>;
}

/** The waits before the second and the third try of a failed import. */
export const RETRY_DELAYS_MS = [5_000, 30_000];

/** Whether the state holds a fence the layer would draw (before any verdict). */
function hasEditableFence(state: EditorState): boolean {
  return mermaidFences(state).some((fence) => {
    const kind = diagramKindOf(fence.body);
    return fence.topLevel && kind !== null && EDITABLE_KINDS.includes(kind);
  });
}

/**
 * Whether an update reached a fenced block: one an edit is in, or one a parse step
 * read for the first time. Only there can an editable fence have come to be.
 */
function touchesFence(update: ViewUpdate): boolean {
  const tree = syntaxTree(update.state);
  const ranges: [number, number][] = [];
  update.changes.iterChangedRanges((_fromA, _toA, fromB, toB) => {
    ranges.push([fromB, toB]);
  });
  const before = syntaxTree(update.startState).length;
  if (tree.length > before) {
    ranges.push([update.changes.mapPos(before, -1), tree.length]);
  }
  return ranges.some(([from, to]) => {
    let found = false;
    tree.iterate({
      from,
      to,
      enter: (node) => {
        found ||= node.name === "FencedCode";
        return !found;
      },
    });
    return found;
  });
}

/** One loader around one import; `diagramLayer` below is the app's single one. */
export function diagramLayerWith(
  load: () => Promise<DiagramChunk>,
): DiagramLayer {
  const compartment = new Compartment();
  let loaded: Extension | null = null;
  let failures = 0;
  let loading: Promise<Extension | null> | null = null;

  /** One try, and the next one after its wait; null when none is left. */
  const attempt = (): Promise<Extension | null> =>
    load().then(
      (chunk) => {
        loaded = chunk.diagramExtension();
        return loaded;
      },
      () => {
        const wait = RETRY_DELAYS_MS[failures];
        failures += 1;
        if (wait === undefined) {
          return null;
        }
        return new Promise<void>((resolve) => {
          setTimeout(resolve, wait);
        }).then(attempt);
      },
    );

  /** Settles once per round: with the layer, or null after the give-up. Never rejects. */
  const request = (): Promise<Extension | null> => {
    loading ??= attempt().then((extension) => {
      if (extension === null) {
        // Not a verdict: a later request starts a fresh round.
        failures = 0;
        loading = null;
      }
      return extension;
    });
    return loading;
  };

  const watcher = ViewPlugin.fromClass(
    class {
      asked = false;
      gone = false;
      readonly view: EditorView;

      constructor(view: EditorView) {
        this.view = view;
        this.check();
      }

      update(update: ViewUpdate): void {
        if (
          !this.asked &&
          (update.docChanged || parseAdvanced(update)) &&
          touchesFence(update)
        ) {
          this.check();
        }
      }

      check(): void {
        if (this.asked || !hasEditableFence(this.view.state)) {
          return;
        }
        this.asked = true;
        void request().then((extension) => {
          if (extension === null) {
            // Gave up: this view may ask again on a later change.
            this.asked = false;
            return;
          }
          if (this.gone) {
            return;
          }
          this.view.dispatch({ effects: compartment.reconfigure(extension) });
        });
      }

      destroy(): void {
        this.gone = true;
      }
    },
  );

  return {
    compartment,
    extension: () => [compartment.of(loaded ?? []), watcher],
    prefetch: async () => {
      await request();
    },
  };
}

const shared = diagramLayerWith(() => import("./index"));

/**
 * The diagram layer for a preview layer. Cheap to call again: every call
 * hands back the one compartment, holding the layer when it is already loaded.
 */
export function diagramLayer(): Extension {
  return shared.extension();
}

/** Warms the chunk, which the tests use. Never rejects. */
export function prefetchDiagrams(): Promise<void> {
  return shared.prefetch();
}
