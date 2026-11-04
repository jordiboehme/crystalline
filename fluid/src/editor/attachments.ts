/**
 * Attaching a file to the document being edited, an engram or a reflection:
 * the pre-check, the upload and the reference that lands in the buffer. Where
 * the bytes go and how the buffer links to them is the asset home's business.
 *
 * Three ways in - paste, drop and the toolbar's file picker - and one flow
 * behind them, because they differ only in where the `File` objects come from.
 * The order is deliberate: refuse locally what the server would refuse anyway
 * (an extension off the allowlist, a file past the ceiling), then pick a free
 * path against what the domain already holds, then upload, and only then write
 * anything into the document. An upload that failed inserts nothing - a
 * reference to bytes that were never stored is a dangling reference, and the
 * maintenance sweep would flag it as one.
 *
 * The insertion is an ordinary transaction through `insertBlock`, so it is
 * collab-safe by construction and lands as one undo step per batch.
 */

import type { Extension } from "@codemirror/state";
import { MapMode, StateEffect, StateField } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { ApiProblem, problemDetail } from "../api/client";
import type { AssetHome } from "../api/files";
import {
  ALLOWED_ATTACHMENT_EXTENSIONS,
  MAX_ATTACHMENT_BYTES,
  freeAttachmentPath,
  isAllowedAttachment,
  isImageAttachment,
} from "../api/files";
import { engramAssetHome } from "./imageFormat";
import { dropLanding, dropPosition } from "./pictures/landing";
import { blockInsertion, insertBlock } from "./toolbar";

/** Said when a file arrives at a buffer whose session has ended. */
const SESSION_ENDED = "The session ended, so the file was not added.";

/** The ceiling in the words an author reads, matching the server's own. */
const CEILING = "10 MiB";

/**
 * Why this file cannot be attached, or null when it can.
 *
 * Both answers are the server's rules, asked here so a picked file is refused
 * in a sentence rather than after a round trip that ends in a failed PUT.
 */
export function refuseAttachment(file: {
  name: string;
  size: number;
}): string | null {
  if (!isAllowedAttachment(file.name)) {
    return `${file.name} is not a kind of file Crystalline stores. Allowed: ${ALLOWED_ATTACHMENT_EXTENSIONS.join(", ")}.`;
  }
  if (file.size > MAX_ATTACHMENT_BYTES) {
    return `${file.name} is larger than ${CEILING}, the most one attachment may hold.`;
  }
  return null;
}

/**
 * The reference one stored file gets: an image is embedded, everything else is
 * linked. The name a reader sees is the one the author's file had, with the
 * two characters that would close a markdown link taken out of it.
 */
export function attachmentMarkdown(name: string, path: string): string {
  const display = name.replace(/[[\]]/g, " ").replace(/\s+/g, " ").trim();
  const text = display === "" ? path : display;
  return isImageAttachment(path) ? `![${text}](${path})` : `[${text}](${path})`;
}

/** The files a clipboard or drag payload carries, and none of its text. */
export function transferFiles(
  transfer: DataTransfer | null | undefined,
): File[] {
  return Array.from(transfer?.files ?? []);
}

/** Sent by a paste or a drop of files: the person chose where they go. */
const placeCaret = StateEffect.define<null>();

/** The user events that mean the person put the caret somewhere. */
const CARET_EVENTS = ["select", "input", "delete", "move", "undo", "redo"];

/**
 * Where the last drop of files lands (`dropLanding`'s end of the block),
 * mapped through the edits since, until the upload's reference goes in.
 * The reference is written there rather than at the caret: the caret the
 * drop sets cannot rest on every block's last line (a code box hides its
 * closing fence line and steps a caret off it), and the drop line promised
 * this place. A place of the person's own after the drop (a click, typing)
 * wins, and so does the caret where a peer deleted the place.
 */
const setDropPlace = StateEffect.define<number | null>();

const dropPlace = StateField.define<number | null>({
  create: () => null,
  update: (place, tr) => {
    for (const effect of tr.effects) {
      if (effect.is(setDropPlace)) return effect.value;
    }
    if (place === null || CARET_EVENTS.some((event) => tr.isUserEvent(event))) {
      return null;
    }
    return tr.changes.mapPos(place, 1, MapMode.TrackDel);
  },
});

/**
 * Whether the person has placed a caret or made a selection in this buffer
 * yet: a click, a key that moves or selects, typing, or a paste or drop of
 * files. A buffer that has only been opened has its caret at the very start,
 * above the page's title, which nobody chose. Changes from the room or from
 * code carry no user event and leave it as it is.
 */
export const caretPlaced = StateField.define<boolean>({
  create: () => false,
  update: (placed, tr) =>
    placed ||
    tr.effects.some((effect) => effect.is(placeCaret)) ||
    CARET_EVENTS.some((event) => tr.isUserEvent(event)),
});

/** What one batch of files needs to become references in one buffer. */
interface UploadBatch {
  files: readonly File[];
  view: EditorView;
  /** A refusal in words an author can act on; null clears a standing one. */
  onError: (message: string | null, kind?: UploadProblemKind) => void;
  /** The instant the dated folder is derived from. Tests pin it. */
  now?: Date;
  /**
   * Runs right before the references are inserted, and may move the caret:
   * a document with lines the reference must not land on steps past them
   * here. Not run when nothing is inserted.
   */
  place?: (view: EditorView) => void;
}

/**
 * One batch and where its files are stored: a home, or the domain whose
 * engram files they become, which is the engram home of that domain.
 */
export type UploadRequest = UploadBatch &
  ({ home: AssetHome; domain?: never } | { domain: string; home?: never });

/**
 * Why something failed, in words: the server's own where the failure came from
 * the API, and the thrown value's otherwise. `catch` binds `unknown`, and a
 * cast would be a promise about what `api()` throws that this module is not
 * the one to make.
 */
function reasonOf(cause: unknown): string {
  return cause instanceof Error ? problemDetail(cause) : String(cause);
}

/**
 * The refusals a name taken since the listing can explain. A 412 says so by
 * itself: it answers only a create-only upload (`If-None-Match: *`) onto a
 * path that holds a file.
 */
const TAKEN_STATUSES = new Set([403, 409, 412]);

/**
 * Store one file at `path`, and when the server refuses because somebody
 * stored a file there since the listing, once more under the next free name.
 *
 * A 412 is a taken name with no further question. For a 403 or 409, whether
 * the name was taken is decided by a fresh listing that now holds the path,
 * never by the refusal's words: a 403 or 409 for any other reason (no right
 * to add, a path the server will not take) is reported as the server sent
 * it. A home that replaces (an engram's) never refuses a taken name, so for
 * it this only ever adds one listing after a refusal. A fresh listing (or
 * the path a 412 refused) goes to `onListed`, so the rest of the batch is
 * named against it.
 */
async function uploadOnce(
  home: AssetHome,
  file: File,
  path: string,
  taken: readonly string[],
  now: Date | undefined,
  onListed: (paths: readonly string[]) => void,
) {
  try {
    return await home.upload(path, file);
  } catch (cause) {
    if (!(cause instanceof ApiProblem) || !TAKEN_STATUSES.has(cause.status)) {
      throw cause;
    }
    if (cause.status === 412) {
      // The refused path counts as taken for the rest of the batch too.
      onListed([path]);
      return home.upload(
        freeAttachmentPath(file.name, [...taken, path], now),
        file,
      );
    }
    let fresh: string[];
    try {
      fresh = (await home.list()).map((row) => row.path);
    } catch {
      throw cause;
    }
    onListed(fresh);
    const wanted = path.toLowerCase();
    if (!fresh.some((held) => held.toLowerCase() === wanted)) {
      throw cause;
    }
    return home.upload(
      freeAttachmentPath(file.name, [...taken, ...fresh], now),
      file,
    );
  }
}

/**
 * Upload every file and insert one reference per stored file, at the cursor.
 *
 * The home's listing is read once up front, because a free path has to be
 * free against what is already there: an engram's upload replaces, so a
 * collision that went unnoticed would silently overwrite somebody's file. A
 * Reflection upload is create-only, so the server refuses a collision, which
 * `uploadOnce` answers. A listing that cannot be read is therefore fatal to
 * the batch rather than treated as an empty domain. A
 * batch whose every file is refused here reads no listing at all, and a
 * buffer that is read-only (a room that has ended) starts nothing.
 *
 * Everything that went wrong is reported once at the end rather than one call
 * per failure: the screen holds a single line, so five dropped files with two
 * refusals have to say both or the author only learns about the last.
 *
 * Resolves with how many references were inserted.
 */
/**
 * What an upload's sentence is about, beyond its words: "unlinked" when the
 * only thing it reports is files that were stored but whose link could not
 * be added because the buffer had gone. A screen that is still there shows
 * the sentence either way; a report made after the screen went needs to
 * know whether the upload finished at all.
 */
export type UploadProblemKind = "unlinked";

export async function uploadAttachments(
  request: UploadRequest,
): Promise<number> {
  const { files, view, onError } = request;
  const home =
    request.home === undefined ? engramAssetHome(request.domain) : request.home;
  onError(null);
  if (files.length === 0) {
    return 0;
  }
  if (view.state.readOnly) {
    onError(SESSION_ENDED);
    return 0;
  }
  const problems: string[] = [];
  const wanted: File[] = [];
  for (const file of files) {
    const refusal = refuseAttachment(file);
    if (refusal === null) {
      wanted.push(file);
    } else {
      problems.push(refusal);
    }
  }
  if (wanted.length === 0) {
    onError(problems.join(" "));
    return 0;
  }
  let taken: string[];
  try {
    taken = (await home.list()).map((row) => row.path);
  } catch (cause) {
    onError(reasonOf(cause));
    return 0;
  }

  const lines: string[] = [];
  const stored: string[] = [];
  for (const file of wanted) {
    // Against this batch as well as against the domain: two files of the same
    // name dropped together must not both claim one path.
    const path = freeAttachmentPath(file.name, taken, request.now);
    try {
      const uploaded = await uploadOnce(
        home,
        file,
        path,
        taken,
        request.now,
        (fresh) => {
          taken = [...taken, ...fresh];
        },
      );
      taken = [...taken, uploaded.path];
      stored.push(uploaded.path);
      lines.push(attachmentMarkdown(file.name, home.linkTo(uploaded.path)));
    } catch (cause) {
      problems.push(reasonOf(cause));
    }
  }

  let inserted = lines.length;
  // The bytes are already stored by the time this runs, so a reference that
  // cannot land is not a no-op the author can shrug off: it leaves a file
  // nothing references, the orphan the maintenance sweep would otherwise
  // raise days later. So every such case ends in a sentence naming the paths.
  // A buffer that turned read-only on the way (a room that ended) gets its
  // own: nobody can link there now. `insertBlock` refuses over a folded
  // frontmatter block and gets the general sentence. A view that was
  // destroyed on the way (its screen remounted or went) would take the
  // dispatch and show nothing; it gets a sentence of its own that does not
  // say "here", since the screen that started the upload may be gone and the
  // sentence is then shown elsewhere (`useAttachmentUploads`).
  // Set when the only problem is the stored files' missing link.
  let unlinked = false;
  if (lines.length > 0) {
    if (view.state.readOnly) {
      inserted = 0;
      unlinked = problems.length === 0;
      problems.push(
        `Uploaded to ${stored.join(", ")}. The session ended, so no link was added. Add it when you edit the page again.`,
      );
    } else {
      const live = view.dom.isConnected;
      // Nobody chose a place yet (the file picker on a buffer only opened):
      // the end of the text, on a line of its own, rather than above the
      // title where the untouched caret sits. A buffer without the field (no
      // paste and drop handlers) keeps the caret it has.
      const atEnd = view.state.field(caretPlaced, false) === false;
      if (live && !atEnd) {
        request.place?.(view);
      }
      if (!live) {
        inserted = 0;
        unlinked = problems.length === 0;
        problems.push(
          `Uploaded to ${stored.join(", ")}, but the screen was closed before the link was added. Add the link when you edit again.`,
        );
      } else if (
        !(atEnd ? appendBlock(view, lines) : placeBlock(view, lines))
      ) {
        inserted = 0;
        problems.push(
          `Uploaded to ${stored.join(", ")}, but the reference could not be inserted here: put the caret in the body and link it.`,
        );
      }
    }
  }
  if (problems.length > 0) {
    onError(problems.join(" "), unlinked ? "unlinked" : undefined);
  }
  return inserted;
}

/**
 * Put `lines` below the block the last drop of files named, when that
 * place still stands, and otherwise below the caret's line (`insertBlock`).
 */
function placeBlock(view: EditorView, lines: readonly string[]): boolean {
  const place = view.state.field(dropPlace, false) ?? null;
  if (place === null) {
    return insertBlock(view, lines);
  }
  const { change, start } = blockInsertion(
    view.state,
    view.state.doc.lineAt(place),
    lines,
  );
  view.dispatch({
    changes: change,
    selection: { anchor: start + (lines[0]?.length ?? 0) },
    userEvent: "input",
  });
  view.focus();
  return true;
}

/**
 * Add `lines` at the end of the text, after one blank line, as one input
 * step, and leave the caret at the end of the first of them. The text keeps
 * ending in one line break. Positions count a line break as one, whatever
 * the separator, as in `insertBlock`.
 */
function appendBlock(view: EditorView, lines: readonly string[]): boolean {
  const { doc, lineBreak } = view.state;
  const last = doc.line(doc.lines);
  const before = doc.lines > 1 ? doc.line(doc.lines - 1) : null;
  // Breaks still needed for a blank line between the text and the block.
  const breaks =
    last.length > 0 ? 2 : before === null || before.length === 0 ? 0 : 1;
  const from = doc.length;
  view.dispatch({
    changes: {
      from,
      insert: `${lineBreak.repeat(breaks)}${lines.join(lineBreak)}${lineBreak}`,
    },
    selection: { anchor: from + breaks + (lines[0]?.length ?? 0) },
    userEvent: "input",
  });
  view.focus();
  return true;
}

/**
 * The two ways a file reaches the buffer without a button: pasted from the
 * clipboard, and dropped onto the text.
 *
 * Only an event actually carrying files is taken. A paste of text and a drag
 * of a selection are the editor's own business and are handed straight back,
 * which is why each handler returns false unless it found something to upload.
 * `dragover` cancels its default for the same reason every drop target does:
 * a browser fires no `drop` at all unless the dragover was prevented.
 */
export function attachmentUploads(onFiles: (files: File[]) => void): Extension {
  return [caretPlaced, dropPlace, handlers(onFiles)];
}

function handlers(onFiles: (files: File[]) => void): Extension {
  return EditorView.domEventHandlers({
    paste(event, view) {
      const files = transferFiles(event.clipboardData);
      // A read-only buffer is the editor's own business: it cancels the paste.
      if (files.length === 0 || view.state.readOnly) {
        return false;
      }
      event.preventDefault();
      // A paste goes where the caret is, so the caret counts as chosen,
      // and a drop's place from before no longer stands.
      view.dispatch({ effects: [placeCaret.of(null), setDropPlace.of(null)] });
      onFiles(files);
      return true;
    },
    dragover(event) {
      // During a dragover Chromium lists no files yet, only the type.
      if (
        transferFiles(event.dataTransfer).length > 0 ||
        Array.from(event.dataTransfer?.types ?? []).includes("Files")
      ) {
        event.preventDefault();
      }
      return false;
    },
    drop(event, view) {
      const files = transferFiles(event.dataTransfer);
      if (files.length === 0) {
        return false;
      }
      event.preventDefault();
      // Taken, so the browser does not open the file in place of the page,
      // but a read-only buffer gets nothing.
      if (view.state.readOnly) {
        return true;
      }
      // Where it was dropped is where it belongs: the caret moves first to
      // where the drop line marked, below the whole block under the pointer
      // (`dropLanding`), so the reference lands there rather than wherever
      // the caret happened to be left, and never inside a table or a
      // paragraph. Where no block takes it, the caret goes to the point
      // itself. A synthetic drop carrying no coordinates leaves the caret
      // where it is instead of asking about a point that is not on the
      // screen. `select.drop` moves the caret without opening a table cell.
      // The reference itself is written at the landing (`dropPlace`).
      const at = dropPosition(view, event);
      const landing = at === null ? null : dropLanding(view.state, at);
      const anchor = landing?.caret ?? at;
      view.dispatch({
        ...(anchor === null
          ? {}
          : { selection: { anchor }, userEvent: "select.drop" }),
        effects: [placeCaret.of(null), setDropPlace.of(landing?.pos ?? null)],
      });
      onFiles(files);
      return true;
    },
  });
}
