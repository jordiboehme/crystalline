/**
 * What this engram carries with it: the files its own prose references.
 *
 * Two questions make one list. The body says which attachments this engram
 * points at, and the domain's listing says which of those actually exist -
 * neither alone is the answer. The domain's whole listing belongs to every
 * engram in it, so showing it here would put somebody else's deck on this
 * page; the body's references alone would claim a file exists because a human
 * typed its name.
 *
 * So a reference the domain does not hold says "missing" rather than
 * disappearing. That is a fact about the knowledge base - the maintenance
 * sweep raises it as a dangling reference, and this panel is where a reader
 * sees the same thing while looking at the engram that made the claim.
 *
 * Removing a file says out loud what it does NOT do. Deleting the bytes leaves
 * every reference in every engram exactly as written, because an app that
 * edited somebody's prose on a delete would be rewriting knowledge to keep its
 * own bookkeeping tidy. The confirm names that consequence and names who will
 * point at it afterwards.
 */

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Trash2 } from "lucide-react";
import { useState } from "react";
import { useMemo } from "react";

import { problemDetail } from "../api/client";
import type { AttachmentRow } from "../api/files";
import {
  attachmentUrl,
  attachmentsKey,
  deleteAttachment,
  listAttachments,
} from "../api/files";
import { assetRefsIn } from "../editor/imageFormat";
import { formatBytes } from "../format";
import { BUTTON, Chip, FOCUS_RING, IconButton } from "./primitives";

export interface AttachmentsSectionProps {
  /** The domain the paths are relative to. */
  domain: string;
  /** The engram's body: its references are what this section lists. */
  body: string;
  /** Whether this session may remove a file. A reader is offered no control. */
  canDelete?: boolean;
}

/** The file's own name, which is what a reader knows it by. */
function nameOf(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/** The extension, lowercase, or "" - the shortest honest word for the kind. */
function kindOf(path: string): string {
  const name = nameOf(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export function AttachmentsSection({
  domain,
  body,
  canDelete = false,
}: AttachmentsSectionProps) {
  const referenced = useMemo(() => assetRefsIn(body), [body]);
  const queryClient = useQueryClient();
  const attachments = useQuery({
    queryKey: attachmentsKey(domain),
    queryFn: () => listAttachments(domain),
    // An engram that references no file asks the server nothing: the listing
    // would answer a question this panel is not going to draw.
    enabled: referenced.length > 0,
  });
  /** Which file the question is open about, by path. */
  const [asking, setAsking] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  if (referenced.length === 0) {
    return null;
  }

  const held = new Map<string, AttachmentRow>(
    (attachments.data ?? []).map((row) => [row.path, row]),
  );

  const remove = (path: string) => {
    void (async () => {
      setRemoving(true);
      setFailure(null);
      try {
        await deleteAttachment(domain, path);
        // The listing this panel drew is stale the moment the file is gone,
        // and it is the same listing the upload flow picks a free path
        // against, so it is refreshed rather than patched in place.
        await queryClient.invalidateQueries({
          queryKey: attachmentsKey(domain),
        });
        setAsking(null);
      } catch (cause) {
        setFailure(
          cause instanceof Error ? problemDetail(cause) : String(cause),
        );
      } finally {
        setRemoving(false);
      }
    })();
  };

  return (
    <section aria-label="Attachments" className="text-sm">
      <h2 className="mb-3 text-caption font-semibold text-slate-500 dark:text-slate-400">
        Attachments
      </h2>
      {failure !== null && (
        <p
          role="alert"
          className="mb-2 text-caption text-red-700 dark:text-red-300"
        >
          {failure}
        </p>
      )}
      <ul className="flex flex-col divide-y divide-slate-100 dark:divide-slate-800">
        {referenced.map((path) => {
          const row = held.get(path);
          const name = nameOf(path);
          const kind = kindOf(path);
          // Missing is a claim, so it waits for an answer: until the listing
          // lands, a file is neither shown as present nor accused of being
          // gone.
          const missing = row === undefined && attachments.isSuccess;
          return (
            <li key={path} className="flex flex-col gap-1 py-2">
              <span className="flex min-w-0 items-center gap-2">
                {row === undefined ? (
                  <span
                    className="min-w-0 truncate text-slate-500 dark:text-slate-400"
                    title={path}
                  >
                    {name}
                  </span>
                ) : (
                  <a
                    href={attachmentUrl(domain, path)}
                    target="_blank"
                    rel="noreferrer"
                    title={path}
                    className={`min-w-0 truncate rounded text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400 ${FOCUS_RING}`}
                  >
                    {name}
                  </a>
                )}
                {canDelete && row !== undefined && asking === null && (
                  // Last in the row and named after the file it acts on: a
                  // rail may hold several of these, and "Remove" alone would
                  // be the same name three times over.
                  <IconButton
                    label={`Remove ${name}`}
                    icon={Trash2}
                    className="ml-auto"
                    onClick={() => {
                      setFailure(null);
                      setAsking(path);
                    }}
                  />
                )}
              </span>
              <span className="flex flex-wrap items-center gap-2 text-caption text-slate-500 dark:text-slate-400">
                {kind !== "" && <Chip>{kind}</Chip>}
                {row !== undefined && (
                  <span className="tabular-nums">{formatBytes(row.size)}</span>
                )}
                {missing && <Chip variant="caution">missing</Chip>}
              </span>
              {asking === path && (
                <span className="flex flex-col gap-2">
                  <span className="text-caption text-slate-600 dark:text-slate-300">
                    {`Remove ${name}? Engrams keep their references until edited; evolve will flag them.`}
                  </span>
                  <span className="flex flex-wrap gap-2">
                    <button
                      type="button"
                      className={BUTTON.destructive}
                      disabled={removing}
                      onClick={() => {
                        remove(path);
                      }}
                    >
                      Remove
                    </button>
                    <button
                      type="button"
                      className={BUTTON.ghost}
                      disabled={removing}
                      onClick={() => {
                        setAsking(null);
                      }}
                    >
                      Cancel
                    </button>
                  </span>
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
