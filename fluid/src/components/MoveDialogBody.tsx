/**
 * Moving an engram: a destination path, prefilled with the file it already
 * lives in, and an optional target domain. The receipt names the address it
 * answers to afterwards, and the caller follows the engram there rather than
 * being left on a page that now 404s.
 *
 * The permalink it will answer to is shown before the move is sent, worked
 * out the way the engine works it out: one in step with the old path follows
 * the file, a custom one stays. When the one that stays would sit in a
 * different folder than the file, the dialog offers "Update the permalink to
 * match", which asks the engine for the path's own - and with the destination
 * left at the current path, that is the in-place repair for a permalink that
 * drifted off its folder. A move rewrites every reference to the engram, and
 * how many it rewrote is handed to the page the author lands on.
 *
 * A move that left attachments behind is the one case where following it
 * immediately would lose something. The receipt's warnings name files the
 * engram still references and the move could not carry across a domain
 * boundary; the move itself succeeded, so there is nothing to retry and no
 * error to raise. This dialog is the only surface still mounted at that
 * moment - the navigation unmounts it - so the notice is held here and the
 * author leaves it themselves.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useState } from "react";
import { useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import { domainTreeKey } from "../api/domain";
import { engramDetailKey } from "../api/engram";
import type { MoveReceipt } from "../api/writes";
import { moveEngram } from "../api/writes";
import { engramRoute } from "../paths";
import {
  defaultMovedPermalink,
  destinationFile,
  pathPermalink,
  permalinkFolder,
} from "../permalink";
import type { MoveDialogProps, MovedState } from "./MoveDialog";

const FIELD_CLASSES =
  "w-full rounded border border-slate-300 bg-transparent px-2 py-1 text-sm focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700";

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
  // The file, not the permalink: the two differ exactly when a permalink has
  // drifted, and prefilling the permalink would move the file to where the
  // drifted address points instead of fixing the address.
  const [destination, setDestination] = useState(
    engram.path?.replace(/\.md$/i, "") ?? engram.permalink,
  );
  const [targetDomain, setTargetDomain] = useState(engram.domain);
  const [matchPermalink, setMatchPermalink] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [warned, setWarned] = useState<MoveReceipt | null>(null);

  const destFile = destinationFile(destination);
  const kept = defaultMovedPermalink(
    engram.permalink,
    engram.path,
    destination,
  );
  const ownPermalink = pathPermalink(destFile);
  // Offered only when the permalink the default keeps names another folder
  // than the file will sit in, which is the drift a folder glob misses.
  const offerMatch =
    destFile !== "" &&
    ownPermalink !== "" &&
    permalinkFolder(kept) !== permalinkFolder(ownPermalink);
  const updatePermalink = offerMatch && matchPermalink;
  const resulting = updatePermalink ? ownPermalink : kept;
  const changesNothing =
    targetDomain === engram.domain &&
    destFile === engram.path &&
    resulting === engram.permalink;

  /** Leave the dialog, refresh what the move changed and follow the engram. */
  const settle = (receipt: MoveReceipt): void => {
    onClose();
    void queryClient.invalidateQueries({
      queryKey: engramDetailKey(engram.domain, engram.permalink),
    });
    // A move is the write that changes the shape of a domain, so the tree
    // the sidebar walks is read again rather than left pointing at where
    // the engram used to be. Both domains when it crossed one: the row left
    // one tree and arrived in the other.
    void queryClient.invalidateQueries({
      queryKey: domainTreeKey(engram.domain),
    });
    if (receipt.domain !== engram.domain) {
      void queryClient.invalidateQueries({
        queryKey: domainTreeKey(receipt.domain),
      });
    }
    // A detail read cached under the NEW address could predate the move when
    // the move only renamed the permalink in place; read it fresh.
    void queryClient.invalidateQueries({
      queryKey: engramDetailKey(receipt.domain, receipt.permalink),
    });
    const state: MovedState | undefined =
      receipt.referencesRewritten > 0
        ? {
            moved: {
              references: receipt.referencesRewritten,
              engrams: receipt.linksRewritten,
            },
          }
        : undefined;
    void navigate(engramRoute(receipt.domain, receipt.permalink), { state });
  };

  const move = useMutation({
    mutationFn: () =>
      moveEngram(engram.domain, {
        permalink: engram.permalink,
        destination,
        ...(targetDomain !== engram.domain
          ? { destination_domain: targetDomain }
          : {}),
        ...(updatePermalink ? { new_permalink: "path" } : {}),
      }),
    onSuccess: (receipt) => {
      if (receipt.attachmentWarnings.length > 0) {
        // The move landed; what it could not carry is the whole message, and
        // travelling now would take it off the screen before it was read.
        setWarned(receipt);
        return;
      }
      settle(receipt);
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
        if (next) {
          return;
        }
        // Escape and the overlay dismiss it too, and after a move that landed
        // they mean the same thing the button does: the engram is at its new
        // address and this page is the old one.
        if (warned !== null) {
          settle(warned);
          return;
        }
        onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Move {engram.title}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            {warned === null
              ? "Every link to it follows it to the new address."
              : "The engram moved. These attachments did not come with it."}
          </Dialog.Description>
          {warned !== null ? (
            <div className="mt-3 flex flex-col gap-3">
              {/*
                Amber on purpose, not the red the form branch gives a refusal:
                the move landed, and this names what it could not carry rather
                than something that failed. The two tones are the difference
                between "go and look at this" and "this did not happen", and
                borrowing the error red would make a move that succeeded read
                as a move that broke.
              */}
              <div
                role="alert"
                className="rounded bg-amber-50 px-2 py-1 text-sm text-amber-900 dark:bg-amber-950 dark:text-amber-100"
              >
                <ul className="list-disc pl-4">
                  {/* Keyed by position as well as text: a warning is a
                      sentence the server composed, not an identity this side
                      can lean on, and two identical sentences would collide as
                      one key. The list arrives whole and is never reordered or
                      filtered, so the position is stable for as long as it is
                      on screen. */}
                  {warned.attachmentWarnings.map((warning, index) => (
                    <li key={`${index}-${warning}`}>{warning}</li>
                  ))}
                </ul>
              </div>
              <div className="flex justify-end">
                <button
                  type="button"
                  autoFocus
                  onClick={() => {
                    settle(warned);
                  }}
                  className={BUTTON_CLASSES}
                >
                  Continue to the engram
                </button>
              </div>
            </div>
          ) : (
            <form
              className="mt-3 flex flex-col gap-3"
              onSubmit={(event) => {
                event.preventDefault();
                if (destFile !== "" && !changesNothing && !move.isPending) {
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
              {offerMatch && (
                <label className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={matchPermalink}
                    onChange={(event) => {
                      setMatchPermalink(event.target.checked);
                    }}
                  />
                  Update the permalink to match
                </label>
              )}
              {/*
                The address it will answer to, said before it is asked for:
                a permalink is what every link and bookmark knows the engram
                by, so a move that changes it should never be a surprise.
              */}
              {ownPermalink !== "" && (
                <p className="text-sm text-slate-600 dark:text-slate-300">
                  <span className={LABEL_CLASSES}>
                    Permalink after the move
                  </span>{" "}
                  <code className="break-all">{resulting}</code>
                </p>
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
                  disabled={destFile === "" || changesNothing || move.isPending}
                  className={BUTTON_CLASSES}
                >
                  Move engram
                </button>
              </div>
            </form>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
