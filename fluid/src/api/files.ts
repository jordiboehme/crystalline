/**
 * Attachments: the bytes an engram teaches with, and the rules for naming
 * them.
 *
 * The naming half is a deliberate mirror of `crates/core/src/attachment.rs`,
 * which is the single source of truth: the extension allowlist, the 10 MiB
 * ceiling and the characters a stored path may not carry. It is mirrored here
 * rather than fetched because it decides what an author is told BEFORE a byte
 * leaves the browser - picking a `.exe` should be a sentence, not a round trip
 * - and every refusal this file makes is one the server would make anyway.
 * When the core table changes, this one changes with it.
 *
 * The sanitizer carries the weight of that mirror. Asset paths are never
 * slugified server-side, so a name that keeps a space, a parenthesis, a `#`, a
 * `%`, a colon, a backslash or a leading dot is REFUSED rather than cleaned up,
 * and an author would see their own screenshot bounce. Everything outside the
 * safe set therefore becomes a dash here.
 */

import { API_BASE, api, encodePermalink, encodeSegment } from "./client";
import { asArray, asNumber, asObject, asString } from "./json";

/** The reserved prefix every attachment lives under, at the domain root. */
export const ASSETS_PREFIX = "assets/";

/** The largest attachment Crystalline stores, 10 MiB, as the core has it. */
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;

/**
 * Every extension the core allowlist maps to a mime, in its own order.
 * `md` is deliberately absent: markdown is an Engram, never an attachment.
 */
export const ALLOWED_ATTACHMENT_EXTENSIONS = [
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "svg",
  "pdf",
  "pptx",
  "odp",
  "docx",
  "odt",
  "xlsx",
  "ods",
  "txt",
  "log",
  "csv",
  "json",
  "yaml",
  "yml",
  "toml",
  "xml",
] as const satisfies readonly string[];

/** The subset that is embedded rather than linked: the core's `image/*`. */
export const IMAGE_ATTACHMENT_EXTENSIONS = [
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "svg",
] as const satisfies readonly string[];

/** What a file picker offers, so the dialog filters what the server accepts. */
export const ATTACHMENT_ACCEPT = ALLOWED_ATTACHMENT_EXTENSIONS.map(
  (extension) => `.${extension}`,
).join(",");

/**
 * The lowercase extension of a filename, or "" when it carries none.
 *
 * The name is trimmed first, because {@link sanitizeAttachmentName} trims too:
 * the question "will this be refused" and the question "what will this be
 * called" have to be asked of the same name, or `"shot.png "` is refused for
 * having no allowlisted extension while the sanitizer would have stored a
 * perfectly ordinary `shot.png`.
 */
function extensionOf(name: string): string {
  const trimmed = name.trim();
  const dot = trimmed.lastIndexOf(".");
  return dot > 0 ? trimmed.slice(dot + 1).toLowerCase() : "";
}

/** Whether the allowlist admits this filename, case-insensitively. */
export function isAllowedAttachment(name: string): boolean {
  return (ALLOWED_ATTACHMENT_EXTENSIONS as readonly string[]).includes(
    extensionOf(name),
  );
}

/** Whether the file is one an engram body embeds rather than links. */
export function isImageAttachment(name: string): boolean {
  return (IMAGE_ATTACHMENT_EXTENSIONS as readonly string[]).includes(
    extensionOf(name),
  );
}

/**
 * The longest stem or extension a sanitized name keeps, in BYTES rather than
 * characters, because the server's 256-byte path ceiling is counted in bytes
 * and a name written in a non-Latin script costs two or three bytes a letter.
 * A hundred each leaves the dated prefix, the separator and a collision suffix
 * a wide margin under that ceiling.
 */
const MAX_NAME_BYTES = 100;

/** What a name with nothing keepable in it is called instead. */
const FALLBACK_STEM = "attachment";

/** How many bytes this text takes on the wire, which is how the server counts. */
function utf8Length(value: string): number {
  return new TextEncoder().encode(value).length;
}

/** The longest prefix of whole characters that fits in `maxBytes`. */
function truncateToBytes(value: string, maxBytes: number): string {
  if (utf8Length(value) <= maxBytes) {
    return value;
  }
  let kept = "";
  let used = 0;
  // Iterating the string yields whole code points, so a truncation never
  // splits a character into an invalid byte sequence.
  for (const character of value) {
    const size = utf8Length(character);
    if (used + size > maxBytes) {
      break;
    }
    kept += character;
    used += size;
  }
  return kept;
}

/**
 * One part of a filename, made safe: lowercase where lowercasing means
 * anything, every run of genuinely unsafe characters collapsed to a single
 * dash, and no leading or trailing dot or dash - a leading dot would be a
 * hidden segment, which the server refuses, and a trailing dash is only noise.
 *
 * Letters and digits of ANY script are kept, because "a filename a human
 * recognizes is the point" and an author writing in Japanese or Greek should
 * get their own words back rather than a row of dashes. What is dropped is
 * what the server refuses or what a path cannot carry: whitespace,
 * parentheses, `#`, `%`, `:`, slashes, backslashes, control and format
 * characters, and symbols including emoji. A name made only of those sanitizes to nothing
 * and takes {@link FALLBACK_STEM}.
 */
function slug(part: string): string {
  const safe = part
    .toLowerCase()
    .replace(/[^\p{L}\p{N}._-]+/gu, "-")
    .replace(/-{2,}/g, "-");
  return truncateToBytes(safe, MAX_NAME_BYTES)
    .replace(/^[.-]+/, "")
    .replace(/[.-]+$/, "");
}

/**
 * A filename an author picked, as a filename the server will accept: lowercase,
 * unsafe characters dashed, extension kept.
 *
 * "Q3 Deck (final).PDF" becomes "q3-deck-final.pdf" and "設計 メモ.png" becomes
 * "設計-メモ.png". A name with nothing safe left in its stem keeps its
 * extension and takes {@link FALLBACK_STEM}, so the result is never a bare
 * extension or an empty string.
 */
export function sanitizeAttachmentName(name: string): string {
  const trimmed = name.trim();
  const dot = trimmed.lastIndexOf(".");
  // A leading dot is part of the stem rather than an extension marker:
  // ".hidden.png" is a hidden file called hidden, not an extension "hidden.png".
  const stem = dot > 0 ? trimmed.slice(0, dot) : trimmed;
  const extension = dot > 0 ? slug(trimmed.slice(dot + 1)) : "";
  const safeStem = slug(stem) || FALLBACK_STEM;
  return extension === "" ? safeStem : `${safeStem}.${extension}`;
}

/** Two digits, so a month sorts and reads the way a folder should. */
function padded(month: number): string {
  return month.toString().padStart(2, "0");
}

/**
 * Where a newly uploaded file goes: the dated default folder
 * `assets/<YYYY>/<MM>/`, under the sanitized name, suffixed `-2`, `-3` until
 * nothing in that folder holds the name already.
 *
 * The date bounds how big one folder gets - a month each - and it narrows the
 * collision scope to that month, so two screenshots called `shot.png` uploaded
 * a year apart are simply two files rather than `shot.png` and `shot-2.png`.
 * The folder is a convention, not a rule: any other path under `assets/` stays
 * valid for a hand-organized or team layout.
 *
 * `existing` is the domain's stored paths; matching is case-insensitive
 * because APFS and NTFS resolve two spellings to one file.
 */
export function freeAttachmentPath(
  name: string,
  existing: readonly string[],
  now: Date = new Date(),
): string {
  const folder = `${ASSETS_PREFIX}${now.getFullYear().toString()}/${padded(
    now.getMonth() + 1,
  )}/`;
  const file = sanitizeAttachmentName(name);
  const dot = file.lastIndexOf(".");
  const stem = dot > 0 ? file.slice(0, dot) : file;
  const suffix = dot > 0 ? file.slice(dot) : "";
  const taken = new Set(existing.map((path) => path.toLowerCase()));
  let candidate = `${folder}${file}`;
  for (let n = 2; taken.has(candidate.toLowerCase()); n += 1) {
    candidate = `${folder}${stem}-${n.toString()}${suffix}`;
  }
  return candidate;
}

/** The path of one attachment on the files route, slashes preserved. */
function filesPath(domain: string, path: string): string {
  return `/domains/${encodeSegment(domain)}/files/${encodePermalink(path)}`;
}

/**
 * The URL an `<img>` or an anchor points at to read the bytes. Absolute from
 * the site root, because it is handed to the browser rather than to
 * {@link api}, which prefixes {@link API_BASE} itself.
 */
export function attachmentUrl(domain: string, path: string): string {
  return `${API_BASE}${filesPath(domain, path)}`;
}

/** The attachment as stored, straight out of the upload's answer. */
export interface UploadedAttachment {
  /** The path to reference it by, which the server decides. */
  path: string;
  /** The mime it will be served under, from the extension allowlist. */
  mime: string;
  size: number;
  sha256: string;
}

/** One attachment a domain carries. */
export interface AttachmentRow extends UploadedAttachment {
  /** Last modification instant, RFC 3339. */
  modified: string;
}

/**
 * Store the bytes, creating or replacing whatever is at `path`.
 *
 * The body is the blob itself - the route reads raw bytes, not an envelope -
 * so a content type is named explicitly: without one the shared client would
 * announce JSON. The server never trusts it either way; the extension decides
 * the mime the file is served under.
 */
export async function uploadAttachment(
  domain: string,
  path: string,
  file: Blob,
): Promise<UploadedAttachment> {
  const payload = await api<unknown>(filesPath(domain, path), {
    method: "PUT",
    headers: { "Content-Type": file.type || "application/octet-stream" },
    body: file,
  });
  const record = asObject(payload);
  return {
    path: asString(record?.path) ?? path,
    mime: asString(record?.mime) ?? "application/octet-stream",
    size: asNumber(record?.size) ?? file.size,
    sha256: asString(record?.sha256) ?? "",
  };
}

/** Remove one attachment. The engram bodies that reference it are untouched. */
export async function deleteAttachment(
  domain: string,
  path: string,
): Promise<void> {
  await api(filesPath(domain, path), { method: "DELETE" });
}

/** Every attachment the domain carries, ordered by path as the server sends it. */
export async function listAttachments(
  domain: string,
): Promise<AttachmentRow[]> {
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/attachments`,
  );
  const record = asObject(payload);
  const rows: AttachmentRow[] = [];
  for (const entry of asArray(record?.attachments)) {
    const row = asObject(entry);
    const path = asString(row?.path);
    if (path === null) {
      continue;
    }
    rows.push({
      path,
      mime: asString(row?.mime) ?? "application/octet-stream",
      size: asNumber(row?.size) ?? 0,
      modified: asString(row?.modified) ?? "",
      sha256: asString(row?.sha256) ?? "",
    });
  }
  return rows;
}

/** The cache key of one domain's attachment listing. */
export function attachmentsKey(domain: string): readonly unknown[] {
  return ["attachments", domain];
}
