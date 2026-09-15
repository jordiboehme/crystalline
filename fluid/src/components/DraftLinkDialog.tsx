/**
 * Sharing one of your own drafts: mint a link, see who is holding one, take
 * one back.
 *
 * A dialog rather than a panel, and that is a decision rather than a default.
 * The editor's right-hand column is not rendered at full width at all
 * (`EngramEditor` drops the `<aside>`), so a sharing surface that lived there
 * would simply be missing for anybody working wide. The trigger sits in the
 * editor's header row, which both widths draw, and the dialog it opens is the
 * same dialog at either measure.
 *
 * The link is readable exactly once, in the reply that mints it - only its
 * hash is stored - so this is the one moment it can be copied, and the dialog
 * says so rather than letting somebody close it and come looking for it later.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useState } from "react";

import { problemDetail } from "../api/client";
import {
  draftLinksKey,
  fetchDraftLinks,
  mintDraftLink,
  revokeDraftLink,
} from "../api/draftLinks";

export interface DraftLinkDialogProps {
  /** The domain the draft lives in. */
  domain: string;
  /** The domain-relative path of the caller's own draft. */
  path: string;
  onClose: () => void;
}

const BUTTON_CLASSES =
  "rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800";

export function DraftLinkDialog({
  domain,
  path,
  onClose,
}: DraftLinkDialogProps): ReactElement {
  const queryClient = useQueryClient();
  const [minted, setMinted] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const links = useQuery({
    queryKey: draftLinksKey(domain, path),
    queryFn: () => fetchDraftLinks(domain, path),
  });

  const refresh = (): void => {
    void queryClient.invalidateQueries({
      queryKey: draftLinksKey(domain, path),
    });
  };

  const mint = useMutation({
    mutationFn: () => mintDraftLink(domain, path),
    onSuccess: (made) => {
      setProblem(null);
      // Held in this component and nowhere else: the token is never readable
      // again, and putting it in the query cache would keep a live credential
      // in memory long after the dialog that showed it is gone.
      setMinted(made.token);
      refresh();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const revoke = useMutation({
    mutationFn: (id: number) => revokeDraftLink(id),
    onSuccess: refresh,
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(34rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-base font-semibold">
            Share this draft
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-600 dark:text-slate-300">
            A link opens this one draft for the first person who uses it, and
            for nobody else. They can read it; editing it is a second step they
            take themselves. You can take the link back at any time, and it ends
            when the draft is folded or discarded.
          </Dialog.Description>

          {problem && (
            <p
              role="alert"
              className="mt-3 rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
            >
              {problem}
            </p>
          )}

          {minted && (
            <div className="mt-3 rounded border border-accent-300 bg-accent-50 p-2 dark:border-accent-700 dark:bg-accent-950">
              <p className="text-sm">
                Copy this link now. It is readable here and never again.
              </p>
              <code
                className="mt-1 block break-all text-xs"
                data-testid="minted-link"
              >
                {`${window.location.origin}/draft/${minted}`}
              </code>
            </div>
          )}

          <div className="mt-4">
            <h3 className="text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">
              Links on this draft
            </h3>
            {links.isPending && (
              <p className="mt-1 text-sm text-slate-500">Reading the links</p>
            )}
            {links.isError && (
              <p
                role="alert"
                className="mt-1 text-sm text-red-700 dark:text-red-300"
              >
                {problemDetail(links.error)}
              </p>
            )}
            {links.data?.length === 0 && (
              <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
                Nobody is holding a link to this draft.
              </p>
            )}
            <ul className="mt-1 flex flex-col gap-1">
              {links.data?.map((link) => (
                <li
                  key={link.id}
                  className="flex flex-wrap items-center justify-between gap-2 text-sm"
                >
                  <span>
                    {link.grantee
                      ? `Held by ${link.grantee}`
                      : "Not opened yet"}
                  </span>
                  <button
                    type="button"
                    disabled={revoke.isPending}
                    onClick={() => {
                      revoke.mutate(link.id);
                    }}
                    className={BUTTON_CLASSES}
                  >
                    Revoke
                  </button>
                </li>
              ))}
            </ul>
          </div>

          <div className="mt-4 flex justify-end gap-2">
            <button type="button" onClick={onClose} className={BUTTON_CLASSES}>
              Close
            </button>
            <button
              type="button"
              disabled={mint.isPending}
              onClick={() => {
                mint.mutate();
              }}
              className={BUTTON_CLASSES}
            >
              Create link
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

export default DraftLinkDialog;
