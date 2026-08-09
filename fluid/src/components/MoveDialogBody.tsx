/**
 * Moving an engram: a destination path, prefilled with where it already
 * lives, and an optional target domain. The receipt names where it landed as
 * a file path, and the caller follows the engram there rather than being
 * left on a page that now 404s.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useState } from "react";
import { useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import { engramDetailKey } from "../api/engram";
import { moveEngram } from "../api/writes";
import { engramRoute } from "../paths";
import type { MoveDialogProps } from "./MoveDialog";

const FIELD_CLASSES =
  "w-full rounded border border-slate-300 bg-transparent px-2 py-1 text-sm focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700";

const LABEL_CLASSES =
  "text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400";

const BUTTON_CLASSES =
  "rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800";

export default function MoveDialogBody({
  engram,
  domains,
  onClose,
}: MoveDialogProps): ReactElement {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [destination, setDestination] = useState(engram.permalink);
  const [targetDomain, setTargetDomain] = useState(engram.domain);
  const [problem, setProblem] = useState<string | null>(null);

  const move = useMutation({
    mutationFn: () =>
      moveEngram(engram.domain, {
        permalink: engram.permalink,
        destination,
        ...(targetDomain !== engram.domain
          ? { destination_domain: targetDomain }
          : {}),
      }),
    onSuccess: (receipt) => {
      onClose();
      void queryClient.invalidateQueries({
        queryKey: engramDetailKey(engram.domain, engram.permalink),
      });
      void navigate(engramRoute(receipt.domain, receipt.permalink));
    },
    onError: (error: Error) => {
      // A 409 (destination taken) and a 422 (reserved name) surface verbatim,
      // the same way every refusal on this app does.
      setProblem(problemDetail(error));
    },
  });

  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        if (!next) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Move {engram.title}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            Inbound bare links are rewritten to follow it.
          </Dialog.Description>
          <form
            className="mt-3 flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              if (destination.trim() !== "" && !move.isPending) {
                setProblem(null);
                move.mutate();
              }
            }}
          >
            {problem && (
              <p
                role="alert"
                className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
              >
                {problem}
              </p>
            )}
            <label className="flex flex-col gap-1 text-sm">
              <span className={LABEL_CLASSES}>Destination path</span>
              <input
                className={FIELD_CLASSES}
                value={destination}
                onChange={(event) => {
                  setDestination(event.target.value);
                }}
                autoFocus
              />
            </label>
            {domains.length > 1 && (
              <label className="flex flex-col gap-1 text-sm">
                <span className={LABEL_CLASSES}>Into domain</span>
                <select
                  className={FIELD_CLASSES}
                  value={targetDomain}
                  onChange={(event) => {
                    setTargetDomain(event.target.value);
                  }}
                >
                  {domains.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className={BUTTON_CLASSES}
              >
                Cancel
              </button>
              <button
                type="submit"
                disabled={destination.trim() === "" || move.isPending}
                className={BUTTON_CLASSES}
              >
                Move engram
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
