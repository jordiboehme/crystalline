/**
 * The picture in place of the reference that names it.
 *
 * An author who pastes a screenshot gets a path in the buffer, and a path is
 * not what they pasted. This draws the file itself where the reference is,
 * from the same files route the reading page reads it from, honoring the same
 * placement fragment - so what the editor shows and what a reader will see
 * are one picture rather than two guesses.
 *
 * The reference is replaced by its picture (an atomic range the caret steps
 * over), and the hidden-mark layer leaves that span alone (`picturesDrawn`,
 * `drawnAsPicture` in `marks/mode.ts`), so exactly one layer owns it. The
 * source comes back in Raw (no picture layer at all) and on the edited line
 * of the show-marks mode, where the picture is drawn under the line instead,
 * as a block. A selection that is exactly one picture is no edit of its line:
 * selecting a picture never swaps the element under the pointer.
 *
 * A `StateField` rather than a `ViewPlugin`, for the reason `fencePreviews`
 * states: CodeMirror refuses block decorations from a plugin's dynamic
 * source, and the edited line's picture is one.
 *
 * References are read off the syntax tree (`pictures/refs.ts`), which is what
 * keeps a path inside a code fence a path: an `Image` node exists where
 * markdown means an image, and nowhere else.
 */

import type { EditorState, Extension, Range } from "@codemirror/state";
import { StateEffect, StateField } from "@codemirror/state";
import type { DecorationSet } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";

import type { AssetResolver } from "../api/files";
import type { ImageFormat } from "./imageFormat";
import { engramAssetHome, imageStyle } from "./imageFormat";
import { markModeChanged, pictureInline, picturesDrawn } from "./marks/mode";
import { parkingChanged } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";
import { altDisplay } from "./pictures/altText";
import type { PictureKind, PictureRef } from "./pictures/refs";
import {
  UNAVAILABLE,
  pictureAssets,
  pictureAt,
  picturesIn,
  selectedPicture,
} from "./pictures/refs";

/** What a picture with no fragment of its own is drawn as. */
const CENTERED: ImageFormat = { align: "center" };

/** The corners a selected attachment shows a handle on, in drawing order. */
const CORNERS = ["nw", "ne", "sw", "se"] as const;

/** The events a widget hands to the editor: a file dragged over or dropped. */
const DRAG_EVENTS = new Set(["dragenter", "dragover", "dragleave", "drop"]);

/** The widget that last drew each picture element, for `updateDOM`. */
const drawnBy = new WeakMap<HTMLElement, PictureWidget>();

/** One picture: in place of its reference, or under its line as a block. */
class PictureWidget extends WidgetType {
  // Assigned in the body rather than declared as constructor parameters: the
  // build erases types, it does not run a TypeScript transform, and parameter
  // properties are syntax that would need one.
  readonly src: string | null;
  readonly format: ImageFormat;
  readonly alt: string;
  readonly kind: PictureKind;
  readonly reason: string | null;
  readonly selected: boolean;
  readonly block: boolean;
  readonly inLink: boolean;

  constructor(ref: PictureRef, alt: string, selected: boolean, block: boolean) {
    super();
    this.src = ref.src;
    this.format = ref.format ?? CENTERED;
    this.alt = alt;
    this.kind = ref.kind;
    this.reason = ref.reason;
    this.selected = selected;
    this.block = block;
    this.inLink = ref.inLink;
  }

  /**
   * Two widgets agreeing on all of this are the same picture, which is what
   * lets a line pushed down the document keep its already-loaded image
   * instead of fetching it again on every Enter above it.
   */
  override eq(other: PictureWidget): boolean {
    return this.sameImage(other) && this.sameFrame(other);
  }

  /** What only a new element can show. */
  sameImage(other: PictureWidget): boolean {
    return (
      other.src === this.src &&
      other.alt === this.alt &&
      other.kind === this.kind &&
      other.reason === this.reason &&
      other.block === this.block &&
      other.inLink === this.inLink
    );
  }

  /** What is patched in place: the frame, the placement and the handles. */
  sameFrame(other: PictureWidget): boolean {
    return (
      other.selected === this.selected &&
      other.format.align === this.format.align &&
      other.format.width === this.format.width
    );
  }

  override toDOM(): HTMLElement {
    const box = document.createElement(this.block ? "div" : "span");
    // A drag always starts on the wrapper, never on the image inside it.
    box.setAttribute("draggable", "true");
    box.dataset["kind"] = this.kind;
    if (this.src === null) {
      // Refused or unavailable: never fetched, the words said instead.
      this.fallback(box, this.reason ?? UNAVAILABLE);
    } else {
      const image = document.createElement("img");
      image.src = this.src;
      image.alt = this.alt;
      image.setAttribute("draggable", "false");
      // A path that points at nothing is an ordinary state while a reference
      // is being typed, and a browser's own broken-image glyph tears the line
      // up. The alt text and one quiet sentence instead, in its place.
      image.addEventListener("error", () => {
        image.remove();
        this.fallback(box, UNAVAILABLE);
      });
      box.appendChild(image);
    }
    this.paint(box);
    drawnBy.set(box, this);
    return box;
  }

  override updateDOM(dom: HTMLElement): boolean {
    const before = drawnBy.get(dom);
    if (!before?.sameImage(this)) {
      return false;
    }
    this.paint(dom);
    drawnBy.set(dom, this);
    return true;
  }

  /** The alt text and the reason, in the muted caption style, before any handle. */
  fallback(box: HTMLElement, reason: string): void {
    const handle = box.querySelector(".cm-picture-handle");
    if (this.alt !== "") {
      const alt = document.createElement("span");
      alt.className = "cm-picture-alt";
      alt.textContent = this.alt;
      box.insertBefore(alt, handle);
    }
    const caption = document.createElement("span");
    caption.className = "cm-image-error";
    caption.textContent = reason;
    box.insertBefore(caption, handle);
  }

  /** The class, the image's placement and the handles, drawn or patched. */
  paint(box: HTMLElement): void {
    box.className = [
      "cm-image-preview",
      "cm-picture",
      ...(this.block ? ["cm-picture-block"] : []),
      ...(this.selected ? ["cm-picture-selected"] : []),
    ].join(" ");
    const image = box.querySelector("img");
    if (image) {
      image.removeAttribute("style");
      Object.assign(
        image.style,
        imageStyle(this.kind === "attachment" ? this.format : CENTERED),
      );
    }
    for (const handle of box.querySelectorAll(".cm-picture-handle")) {
      handle.remove();
    }
    if (this.selected && this.kind === "attachment") {
      for (const corner of CORNERS) {
        const handle = document.createElement("span");
        handle.className = "cm-picture-handle";
        handle.dataset["corner"] = corner;
        handle.setAttribute("aria-hidden", "true");
        handle.setAttribute("draggable", "false");
        box.appendChild(handle);
      }
    }
  }

  /**
   * A file dragged over or dropped onto a picture is the editor's (the upload
   * handlers see it). A press on a linked picture is the link's: its popover
   * opens as for the link's words. Everything else on a picture is the
   * picture layer's own, through native listeners (`pointerPlugin`).
   */
  override ignoreEvent(event: Event): boolean {
    if (DRAG_EVENTS.has(event.type)) {
      return false;
    }
    return !(this.inLink && event.type === "mousedown");
  }
}

/** The references a document has, and how they are drawn now. */
interface Drawn {
  refs: readonly PictureRef[];
  decorations: DecorationSet;
  /** The pictures drawn in place: what the caret steps over whole. */
  inline: DecorationSet;
}

function decorate(state: EditorState, refs: readonly PictureRef[]): Drawn {
  const selected = selectedPicture(state);
  const all: Range<Decoration>[] = [];
  const inline: Range<Decoration>[] = [];
  for (const ref of refs) {
    const alt = altDisplay(state.doc.sliceString(ref.altFrom, ref.altTo));
    const chosen = selected?.from === ref.from && selected.to === ref.to;
    if (pictureInline(state, ref)) {
      const range = Decoration.replace({
        widget: new PictureWidget(ref, alt, chosen, false),
        inclusive: false,
      }).range(ref.from, ref.to);
      all.push(range);
      inline.push(range);
    } else {
      // The edited line shows its source; the picture goes under the line.
      all.push(
        Decoration.widget({
          widget: new PictureWidget(ref, alt, chosen, true),
          block: true,
          side: 1,
        }).range(state.doc.lineAt(ref.to).to),
      );
    }
  }
  return {
    refs,
    // Two pictures on one line produce two block widgets at the same
    // position, which is a legal set only when it is sorted for us.
    decorations: Decoration.set(all, true),
    inline: Decoration.set(inline, true),
  };
}

function build(state: EditorState): Drawn {
  return decorate(
    state,
    // A table's pictures are its cells', drawn by the grid.
    picturesIn(state).filter((ref) => !ref.inCell),
  );
}

/**
 * The picture under a press: the inline one whose element it is, or the
 * block one in that place among its line's block pictures.
 */
function pressedPicture(
  view: EditorView,
  box: HTMLElement,
  refs: readonly PictureRef[],
): PictureRef | null {
  const { state } = view;
  const at = view.posAtDOM(box);
  if (!box.classList.contains("cm-picture-block")) {
    // One in: at a junction of two pictures, the right one is this one.
    const ref = pictureAt(state, at + 1);
    return ref?.from === at ? ref : null;
  }
  let index = 0;
  for (
    let before = box.previousElementSibling;
    before?.classList.contains("cm-picture-block") === true;
    before = before.previousElementSibling
  ) {
    index += 1;
  }
  const line = state.doc.lineAt(at).number;
  const below = refs.filter(
    (ref) =>
      state.doc.lineAt(ref.to).number === line && !pictureInline(state, ref),
  );
  return below[index] ?? null;
}

/**
 * A plain press on a picture selects it whole. Native listeners on
 * `contentDOM`, because the widget keeps its events from CodeMirror's own
 * handlers. The press keeps its default, which a drag of the wrapper needs;
 * a drag that starts on a handle is cancelled (the handles resize).
 */
function pointerPlugin(field: StateField<Drawn>): Extension {
  return ViewPlugin.fromClass(
    class {
      readonly view: EditorView;
      /** Whether the last press began on a handle. */
      onHandle = false;

      readonly mousedown = (event: MouseEvent): void => {
        const target = event.target;
        if (!(target instanceof Element)) {
          return;
        }
        this.onHandle = target.closest(".cm-picture-handle") !== null;
        if (
          this.onHandle ||
          event.button !== 0 ||
          event.shiftKey ||
          event.metaKey ||
          event.ctrlKey ||
          event.altKey
        ) {
          return;
        }
        const box = target.closest<HTMLElement>(".cm-picture");
        if (!box || box.closest(".cm-content") !== this.view.contentDOM) {
          return;
        }
        const ref = pressedPicture(
          this.view,
          box,
          this.view.state.field(field).refs,
        );
        // A linked picture is the link's: its popover opens instead.
        if (!ref || ref.inLink) {
          return;
        }
        this.view.dispatch({
          selection: { anchor: ref.from, head: ref.to },
          userEvent: "select.pointer",
        });
        this.view.focus();
      };

      readonly dragstart = (event: DragEvent): void => {
        // A browser names the draggable wrapper as the target, so the press
        // says whether the drag began on a handle.
        const target = event.target;
        if (
          this.onHandle ||
          (target instanceof Element &&
            target.closest(".cm-picture-handle") !== null)
        ) {
          event.preventDefault();
        }
      };

      constructor(view: EditorView) {
        this.view = view;
        view.contentDOM.addEventListener("mousedown", this.mousedown);
        view.contentDOM.addEventListener("dragstart", this.dragstart);
      }

      destroy() {
        this.view.contentDOM.removeEventListener("mousedown", this.mousedown);
        this.view.contentDOM.removeEventListener("dragstart", this.dragstart);
      }
    },
  );
}

/**
 * The pictures' own chrome.
 *
 * `.cm-picture` and its parts are fresh classes with no competing rule
 * anywhere, written by this module alone, so the cascade trap the fence
 * previews document (base style modules mount REVERSED, equal specificity
 * decided by that order) has nothing to bite on. The caption is the muted
 * slate the fence previews use, for the same reason: a half-typed path is
 * "broken" almost continuously, so this is a hint rather than an alarm.
 */
const previewTheme = EditorView.baseTheme({
  ".cm-picture": { position: "relative" },
  ".cm-image-preview.cm-picture-block": {
    display: "block",
    padding: "0.5rem 0.75rem",
  },
  ".cm-picture-alt": { marginRight: "0.5em" },
  ".cm-picture-selected > img, .cm-picture-selected > .cm-image-error": {
    outline: "2px solid var(--color-accent-600)",
    outlineOffset: "2px",
  },
  "&dark .cm-picture-selected > img, &dark .cm-picture-selected > .cm-image-error":
    { outlineColor: "var(--color-accent-400)" },
  ".cm-picture-handle": {
    position: "absolute",
    width: "0.5rem",
    height: "0.5rem",
    background: "var(--color-accent-600)",
    border: "1px solid white",
    borderRadius: "1px",
  },
  ".cm-picture-handle[data-corner=nw]": { top: "-0.25rem", left: "-0.25rem" },
  ".cm-picture-handle[data-corner=ne]": { top: "-0.25rem", right: "-0.25rem" },
  ".cm-picture-handle[data-corner=sw]": {
    bottom: "-0.25rem",
    left: "-0.25rem",
  },
  ".cm-picture-handle[data-corner=se]": {
    bottom: "-0.25rem",
    right: "-0.25rem",
  },
  ".cm-image-error": {
    color: "var(--color-slate-500)",
    fontSize: "var(--text-caption, 0.75rem)",
    lineHeight: "var(--text-caption--line-height, 1rem)",
  },
  "&dark .cm-image-error": { color: "var(--color-slate-400)" },
});

/**
 * The files of each domain whose bytes changed since the editor first listed
 * them, path to sha256, read by {@link imagePreviews} for a domain it is
 * handed. One map per domain, updated in place by {@link setPreviewVersions}
 * so a preview extension built earlier sees the update.
 */
const previewVersions = new Map<string, Map<string, string>>();

/** The map behind a domain's previews, created on first use. */
function versionsOf(domain: string): Map<string, string> {
  let found = previewVersions.get(domain);
  if (found === undefined) {
    found = new Map();
    previewVersions.set(domain, found);
  }
  return found;
}

/**
 * Tells the previews of `domain` which files changed on disk since the
 * editor first listed them; send {@link refreshPreviews} to the view after
 * it so the pictures are drawn anew. An empty map forgets them.
 */
export function setPreviewVersions(
  domain: string,
  versions: ReadonlyMap<string, string>,
): void {
  const live = versionsOf(domain);
  live.clear();
  for (const [path, sha] of versions) live.set(path, sha);
}

/** Rebuilds every preview, for when {@link setPreviewVersions} changed. */
export const refreshPreviews = StateEffect.define<null>();

/**
 * The pictures a document references, drawn in place: a domain's engram
 * files when handed a domain, or whatever the resolver resolves. Provides
 * that resolver as the page's `pictureAssets`.
 */
export function imagePreviews(source: string | AssetResolver): Extension {
  const assets =
    typeof source === "string"
      ? engramAssetHome(source, versionsOf(source))
      : source;
  const field = StateField.define<Drawn>({
    create: (state) => build(state),
    update: (value, tr) => {
      if (
        tr.docChanged ||
        parseAdvanced(tr) ||
        tr.effects.some((effect) => effect.is(refreshPreviews)) ||
        tr.startState.facet(pictureAssets) !== tr.state.facet(pictureAssets)
      ) {
        return build(tr.state);
      }
      // The references stand where they stood: only how they are drawn may
      // change (selected, or the edited line moved).
      return tr.selection !== undefined ||
        markModeChanged(tr) ||
        parkingChanged(tr)
        ? decorate(tr.state, value.refs)
        : value;
    },
    provide: (f) => [
      EditorView.decorations.from(f, (drawn) => drawn.decorations),
      EditorView.atomicRanges.of((view) => view.state.field(f).inline),
    ],
  });
  return [
    pictureAssets.of(assets),
    picturesDrawn.of(true),
    field,
    pointerPlugin(field),
    previewTheme,
  ];
}
