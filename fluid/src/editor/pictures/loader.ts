/**
 * The eager stub for the lazy picture tools: the keys around a picture now,
 * the drop line, the drag-move and the resize handles later.
 *
 * The shape of the grid's loader (`grid/loader.ts`). Nothing here imports
 * `./index` statically: the chunk is asked for with a dynamic import, the
 * first time a view's syntax tree holds an `Image` node or a file is dragged
 * over a view (or a screen calls `prefetchPictureTools`). One compartment and
 * one load for the whole app - `previewConfig` runs again on every Raw flip,
 * and a compartment made per call would forget what the chunk brought.
 *
 * Until the chunk is in, nothing here changes the editor: the pictures are
 * drawn and a press selects one (`imagePreviews`), the hidden-mark layer
 * keeps its own keys around them. A failed import is tried again after 5
 * seconds, then after 30; after the third failure the loader gives up
 * quietly and the views stay as they are, with no tools.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension } from "@codemirror/state";
import { Compartment } from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import { ViewPlugin } from "@codemirror/view";

interface PictureChunk {
  pictureToolsExtension: () => Extension;
}

export interface PictureTools {
  /** The extension to place in a state: the compartment and its watcher. */
  extension: () => Extension;
  compartment: Compartment;
  /** Starts the load; resolves when it settled, whichever way. */
  prefetch: () => Promise<void>;
}

/** Whether the tree holds an `Image` node anywhere. */
function hasImage(state: EditorState): boolean {
  let found = false;
  syntaxTree(state).iterate({
    enter: (node) => {
      if (found) return false;
      if (node.name === "Image") found = true;
      return !found;
    },
  });
  return found;
}

/** Whether a drag carries files from outside the page. */
function carriesFiles(event: DragEvent): boolean {
  return Array.from(event.dataTransfer?.types ?? []).includes("Files");
}

/** The waits before the second and the third try of a failed import. */
export const RETRY_DELAYS_MS = [5_000, 30_000];

/** One loader around one import; `pictureTools` below is the app's single one. */
export function pictureToolsWith(
  load: () => Promise<PictureChunk>,
): PictureTools {
  const compartment = new Compartment();
  let loaded: Extension | null = null;
  let failures = 0;
  let loading: Promise<Extension | null> | null = null;

  /** One try, and the next one after its wait; null when none is left. */
  const attempt = (): Promise<Extension | null> =>
    load().then(
      (chunk) => {
        loaded = chunk.pictureToolsExtension();
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

  /** Settles once: with the tools, or with null. Never rejects. */
  const request = (): Promise<Extension | null> => {
    loading ??= attempt();
    return loading;
  };

  const watcher = ViewPlugin.fromClass(
    class {
      asked = false;
      gone = false;
      readonly view: EditorView;

      readonly dragenter = (event: DragEvent): void => {
        if (carriesFiles(event)) this.ask();
      };

      constructor(view: EditorView) {
        this.view = view;
        view.dom.addEventListener("dragenter", this.dragenter);
        this.check();
      }

      update(update: ViewUpdate): void {
        if (
          update.docChanged ||
          update.viewportChanged ||
          syntaxTree(update.startState) !== syntaxTree(update.state)
        ) {
          this.check();
        }
      }

      check(): void {
        if (this.asked) return;
        if (loaded !== null) {
          // The compartment already holds the tools (a view built after the
          // load, mostly after the mount prefetch): nothing to ask, no walk.
          this.asked = true;
          return;
        }
        if (hasImage(this.view.state)) this.ask();
      }

      ask(): void {
        if (this.asked) return;
        this.asked = true;
        void request().then((extension) => {
          if (this.gone || extension === null) return;
          this.view.dispatch({ effects: compartment.reconfigure(extension) });
        });
      }

      destroy(): void {
        this.gone = true;
        this.view.dom.removeEventListener("dragenter", this.dragenter);
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

const shared = pictureToolsWith(() => import("./index"));

/**
 * The picture tools for a preview layer. Cheap to call again: every call
 * hands back the one compartment, holding the tools when they are already
 * loaded.
 */
export function pictureTools(): Extension {
  return shared.extension();
}

/** Warms the chunk; screens call it on mount. Never rejects. */
export function prefetchPictureTools(): Promise<void> {
  return shared.prefetch();
}
