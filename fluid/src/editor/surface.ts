/**
 * The one editor surface both editors mount: the engram editor and the
 * Reflection editor build every buffer from this list, and what differs
 * between them comes in through a document adapter (`adapter.ts`).
 *
 * A screen still owns what is only its own: the line separator, the room
 * binding, the save keymap, a compartment it reconfigures later. Those come
 * in through `screen` and sit right after the base extensions, where they
 * always stood.
 */

import { autocompletion } from "@codemirror/autocomplete";
import type { Compartment, Extension } from "@codemirror/state";

import type { DocumentAdapter } from "./adapter";
import type { ElementFace } from "./elementContext";
import { elementContextListener } from "./elementContext";
import { elementPreviews } from "./elementPreviews";
import { fenceMono } from "./fenceMono";
import { fencePreviews } from "./fencePreviews";
import type { FloatingContext } from "./floatingContext";
import { floatingContextListener } from "./floatingContext";
import { cellPreviewLayers } from "./grid/draws";
import { tableGrid } from "./grid/loader";
import { imageContextListener } from "./imageVerbs";
import { hiddenMarkLayer } from "./marks";
import { marksOnEditLine } from "./marks/mode";
import { livePreview } from "./preview";
import { readingTheme } from "./readingTheme";
import { richPaste } from "./richPaste";
import { RAW_MONO, baseExtensions } from "./setup";
import { slashCompletions } from "./slashMenu";
import type { TableUiContext } from "./tableVerbs";
import { tableContextListener } from "./tableVerbs";
import { formattingKeymap } from "./toolbar";

/** Everything one buffer on either editor is built from. */
export interface SurfaceOptions {
  /** What this kind of document adds. */
  adapter: DocumentAdapter;
  dark: boolean;
  /**
   * CodeMirror's own undo history. Off in a room, whose own undo stack
   * replaces it: see `baseExtensions`.
   */
  history: boolean;
  /**
   * What only this screen adds, placed right after the base extensions: the
   * room binding and its undo keymap, the save keymap, a compartment the
   * screen reconfigures later.
   */
  screen: Extension[];
  /** The layer switch the engram editor's Raw toggle reconfigures. */
  preview: Compartment;
  /** Whether the buffer opens as the source view, with no preview. */
  raw: boolean;
  /**
   * The image previews, built by the screen because it knows where the files
   * load from (a domain, or a Reflection's own files in a compartment of
   * their own). Inside the preview: Raw shows the reference, not the picture.
   */
  images: Extension;
  /**
   * Paste and drop uploads. Inside the options for the same reason the table
   * listener is: every state a screen builds goes through this function, and
   * handlers added beside it at one site would go silent the first time a
   * session rebuilt the buffer.
   */
  uploads: Extension;
  /**
   * Told when the caret enters or leaves a table, so the format bar can draw
   * its table verbs. It travels inside the options rather than being added
   * beside them, because every state a screen builds goes through this
   * function and a listener added at only one site would go silent the first
   * time a session rebuilt the buffer.
   */
  onTableContext: (context: TableUiContext) => void;
  /**
   * Told when the caret enters or leaves an attachment image, so the format
   * bar can draw its image menu. It travels with the table listener above and
   * for the same reason.
   */
  onImageContext: (onImage: boolean) => void;
  /**
   * Told when the caret enters or leaves a callout or a status label, or
   * changes its kind, fold or colour, so the format bar can draw the
   * segment for it. It travels with the listeners above for their reason.
   */
  onElementContext: (face: ElementFace) => void;
  /**
   * Told what the floating action bar stands by: a text selection, the open
   * table cell, or nothing. It travels with the listeners above for their
   * reason.
   */
  onFloatingContext: (context: FloatingContext) => void;
  /**
   * The compartment the screen reconfigures when the marks setting flips. It
   * sits outside the preview compartment, so a flip never rebuilds Raw or the
   * previews. Absent, the setting is a bare value for the life of the state.
   */
  marks?: Compartment;
  /** Whether marks show on the line being edited. Absent means hidden. */
  showMarks?: boolean;
}

/**
 * What the preview compartment holds, in the one place it is ever spelled.
 *
 * Two sites configure that compartment - `surfaceExtensions`, which every
 * state goes through, and the engram editor's Raw toggle - and a decoration
 * layer added to only one of them would vanish on whichever path was missed.
 * Later layers are appended to this array and reach both at once.
 */
export function previewConfig(
  off: boolean,
  dark: boolean,
  images: Extension,
  adapter: DocumentAdapter,
): Extension {
  // The prose layers the main view gets. The cell editor of the table grid
  // draws with the same ones, read through `cellPreviewLayers`.
  const proseLayers = [
    elementPreviews({ sections: adapter.kind === "reflection" }),
    livePreview(),
    ...adapter.previewLayers,
  ];
  return off
    ? // Raw is deliberately the source view, so the whole buffer goes back to
      // mono: the shared theme sets prose proportional, and this is the one
      // place that undoes it.
      [RAW_MONO]
    : [
        // Before the mark folding, so a callout head, a label or a formula
        // replaces its whole span over the marks folded inside it.
        ...proseLayers,
        cellPreviewLayers.of(proseLayers),
        // After the prose layers and outside `livePreview()`, so the cell
        // editor, which reads `proseLayers`, does not get it until it is
        // given the layer on purpose.
        hiddenMarkLayer(),
        // Before the read-only previews: the grid takes over the tables it
        // draws once its chunk is in.
        tableGrid(),
        fencePreviews(dark),
        // A pasted screenshot is a path in the buffer and a picture on the
        // page, so the picture is drawn here too: an author placing an image
        // sees what a reader will see rather than the reference to it.
        images,
        // Prose is proportional, code is not. Raw mode does not need this -
        // the whole buffer is mono there - so it rides the preview branch
        // with the rest of the read model.
        fenceMono(),
      ];
}

/**
 * The whole shared extension set of one buffer.
 *
 * `setState` replaces the configuration entirely, so anything a screen lists
 * beside this rather than through it silently disappears the first time a
 * session rebuilds the buffer.
 */
export function surfaceExtensions(options: SurfaceOptions): Extension[] {
  const { adapter } = options;
  return [
    ...baseExtensions(options.dark, { history: options.history }),
    ...options.screen,
    // The reading size and no inset, so text and images sit on the column's
    // own left edge, under the toolbar.
    readingTheme,
    // What the format bar's context segment watches. Outside the preview
    // compartment: a table is a table in Raw mode too.
    tableContextListener(options.onTableContext),
    // The same watch for the image menu. Outside the preview compartment
    // beside it: the menu edits the target's text, which is as much text in
    // Raw mode as it is under the previews.
    imageContextListener(options.onImageContext),
    // And for the callout and status segments: a callout's kind is text in
    // Raw mode too.
    elementContextListener(options.onElementContext),
    // And for the floating action bar. Outside the preview compartment: a
    // selection is a selection in Raw mode too, and the screen decides
    // there whether the bar draws.
    floatingContextListener(options.onFloatingContext),
    // A pasted screenshot and a dropped deck are attachments in Raw mode too,
    // so these sit outside the compartment beside it.
    options.uploads,
    // Rich text from another app arrives as Markdown, in Raw mode too: the
    // buffer is Markdown either way. It runs before the uploads above
    // (`Prec.high`): HTML with text and only images beside it (a word
    // processor's picture of the copy) is converted and nothing is uploaded;
    // any other file, or images with no text, goes to the uploads.
    richPaste(),
    // The toolbar's own shortcuts. Outside the preview compartment: Mod-b and
    // Mod-i are typing help like the completions below, and raw mode is
    // still markdown being written. Its `Prec.high` wrapper - not its
    // position here - is what beats defaultKeymap's own Mod-i; see
    // `formattingKeymap`.
    formattingKeymap,
    // Typing help rather than preview, so it stays on in raw mode: outside
    // the compartment the Raw toggle empties. One `autocompletion` for every
    // source, because two would fight over the override. The `/` menu comes
    // first and is on both kinds of document; it answers only a slash that
    // starts a line, where no other source does.
    autocompletion({ override: [slashCompletions, ...adapter.completions] }),
    ...adapter.extensions,
    // The marks setting. Outside the preview compartment: a flip dispatches
    // `marks.reconfigure(...)` alone and never rebuilds Raw or the previews.
    options.marks
      ? options.marks.of(marksOnEditLine.of(options.showMarks ?? false))
      : marksOnEditLine.of(options.showMarks ?? false),
    // Every later flip goes through the compartment rather than through this
    // array, which is read once per state.
    options.preview.of(
      previewConfig(options.raw, options.dark, options.images, adapter),
    ),
  ];
}
