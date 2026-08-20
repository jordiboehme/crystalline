/**
 * Attaching a file to the engram being edited: the pre-check, the upload and
 * the reference that lands in the buffer.
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
import { EditorView } from "@codemirror/view";

import { problemDetail } from "../api/client";
import {
  ALLOWED_ATTACHMENT_EXTENSIONS,
  MAX_ATTACHMENT_BYTES,
  freeAttachmentPath,
  isAllowedAttachment,
  isImageAttachment,
  listAttachments,
  uploadAttachment,
} from "../api/files";
import { insertBlock } from "./toolbar";

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

/** What one batch of files needs to become references in one buffer. */
export interface UploadRequest {
  domain: string;
  files: readonly File[];
  view: EditorView;
  /** A refusal in words an author can act on; null clears a standing one. */
  onError: (message: string | null) => void;
  /** The instant the dated folder is derived from. Tests pin it. */
  now?: Date;
}

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
 * Upload every file and insert one reference per stored file, at the cursor.
 *
 * The domain's listing is read once up front, because a free path has to be
 * free against what is already there: uploading is a create-or-REPLACE, so a
 * collision that went unnoticed would silently overwrite somebody's file. A
 * listing that cannot be read is therefore fatal to the batch rather than
 * treated as an empty domain.
 *
 * Everything that went wrong is reported once at the end rather than one call
 * per failure: the screen holds a single line, so five dropped files with two
 * refusals have to say both or the author only learns about the last.
 *
 * Resolves with how many references were inserted.
 */
export async function uploadAttachments(
  request: UploadRequest,
): Promise<number> {
  const { domain, files, view, onError } = request;
  onError(null);
  if (files.length === 0) {
    return 0;
  }
  const problems: string[] = [];
  let taken: string[];
  try {
    taken = (await listAttachments(domain)).map((row) => row.path);
  } catch (cause) {
    onError(reasonOf(cause));
    return 0;
  }

  const lines: string[] = [];
  const stored: string[] = [];
  for (const file of files) {
    const refusal = refuseAttachment(file);
    if (refusal !== null) {
      problems.push(refusal);
      continue;
    }
    // Against this batch as well as against the domain: two files of the same
    // name dropped together must not both claim one path.
    const path = freeAttachmentPath(file.name, taken, request.now);
    try {
      const uploaded = await uploadAttachment(domain, path, file);
      taken = [...taken, uploaded.path];
      stored.push(uploaded.path);
      lines.push(attachmentMarkdown(file.name, uploaded.path));
    } catch (cause) {
      problems.push(reasonOf(cause));
    }
  }

  let inserted = lines.length;
  // The bytes are already stored by the time this runs, so a refused insertion
  // is not a no-op the author can shrug off: it leaves a file in the domain
  // that no engram references, which is the orphan the maintenance sweep would
  // otherwise raise days later. `insertBlock` refuses over a folded
  // frontmatter block, and a dispatch into a view that has been destroyed -
  // the buffer was rebuilt while the upload was in flight - lands nowhere
  // either. Both end here, naming the paths so they can be linked by hand or
  // deleted.
  if (lines.length > 0 && !insertBlock(view, lines)) {
    inserted = 0;
    problems.push(
      `Uploaded to ${stored.join(", ")}, but the reference could not be inserted here: put the caret in the body and link it, or delete the file.`,
    );
  }
  if (problems.length > 0) {
    onError(problems.join(" "));
  }
  return inserted;
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
  return EditorView.domEventHandlers({
    paste(event) {
      const files = transferFiles(event.clipboardData);
      if (files.length === 0) {
        return false;
      }
      event.preventDefault();
      onFiles(files);
      return true;
    },
    dragover(event) {
      if (transferFiles(event.dataTransfer).length > 0) {
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
      // Where it was dropped is where it belongs: the caret moves there first,
      // so the reference lands under the pointer rather than wherever the
      // caret happened to be left. A synthetic drop carrying no coordinates
      // leaves the caret where it is instead of asking about a point that is
      // not on the screen.
      const at =
        Number.isFinite(event.clientX) && Number.isFinite(event.clientY)
          ? view.posAtCoords({ x: event.clientX, y: event.clientY })
          : null;
      if (at !== null) {
        view.dispatch({ selection: { anchor: at } });
      }
      onFiles(files);
      return true;
    },
  });
}
