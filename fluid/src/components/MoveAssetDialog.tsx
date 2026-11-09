/**
 * Move or rename one of the Reflection's files. Every page that links it is
 * rewritten by the server, so the whole Reflection is read again after a
 * move. A file that a page open in a room links is refused until the room is
 * closed, and that sentence is shown as the server wrote it.
 *
 * Lazy: the reading page loads it the first time a curator presses Move.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useEffect, useId, useRef, useState } from "react";

import { problemDetail } from "../api/client";
import { ASSETS_PREFIX } from "../api/files";
import type { AssetMoveReceipt } from "../api/reflection";
import { moveReflectionAsset, reflectionAssetsKey } from "../api/reflection";
import { invalidateReflection } from "../api/reflectionDomains";
import { focusPageHeading } from "../focusHeading";
import { BUTTON, FIELD } from "./primitives";
import { refreshAfterWrite } from "../query/refreshAfterWrite";

/** "Moved. 2 links updated.", with one link said as one. */
function movedText(links: number): string {
  return `Moved. ${links} ${links === 1 ? "link" : "links"} updated.`;
}

/** The extension, lowercase, or "". */
function extensionOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/**
 * Why a new path is not sent, or null. A path that leaves `assets/` or has
 * an empty part is refused here, before any request; the server would
 * refuse it too. A file keeps its extension: the server serves the bytes
 * under the type the extension names, so PNG bytes moved to `.jpg` would be
 * served as a JPEG.
 */
function refusalOf(target: string, from: string): string | null {
  if (target === "") {
    return null;
  }
  if (
    target.startsWith("/") ||
    target.split("/").some((part) => part === "" || part === "..")
  ) {
    return "Write a path like diagrams/flow.png.";
  }
  const kept = extensionOf(from);
  if (extensionOf(target) !== kept) {
    return kept === ""
      ? "Keep the name without an extension."
      : `Keep the .${kept} extension.`;
  }
  return null;
}

export function MoveAssetDialog({
  domain,
  path,
  onClose,
}: {
  domain: string;
  /** The file's stored path, `assets/...`. */
  path: string;
  onClose: () => void;
}): ReactElement {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const from = path.startsWith(ASSETS_PREFIX)
    ? path.slice(ASSETS_PREFIX.length)
    : path;
  const [to, setTo] = useState(from);
  const [receipt, setReceipt] = useState<AssetMoveReceipt | null>(null);
  const field = useId();
  const queryClient = useQueryClient();
  const closeButton = useRef<HTMLButtonElement | null>(null);
  const move = useMutation({
    mutationFn: () =>
      moveReflectionAsset(domain, path, ASSETS_PREFIX + to.trim()),
    onSuccess: (answer) => {
      // Every page that linked the file was rewritten and the listing moved.
      // The listing's key sits under the Reflection's prefix already; it is
      // named as well so this stays right if the keys are ever split.
      invalidateReflection(queryClient, domain);
      void refreshAfterWrite(queryClient, {
        queryKey: reflectionAssetsKey(domain),
      });
      setReceipt(answer);
    },
  });
  useEffect(() => {
    if (receipt !== null) {
      closeButton.current?.focus();
    }
  }, [receipt]);
  const target = to.trim();
  const refusal = refusalOf(target, from);
  const sendable = target !== "" && target !== from && refusal === null;
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
        <Dialog.Content
          onCloseAutoFocus={(event) => {
            // The Move button may be gone with the old row; the page's
            // heading is a place that is always there.
            if (receipt !== null) {
              event.preventDefault();
              focusPageHeading();
            }
          }}
          className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900"
        >
          <Dialog.Title className="text-lg font-semibold">{`Move ${name}`}</Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            Links to it in every page follow.
          </Dialog.Description>
          <form
            className="mt-3 flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              if (receipt === null && sendable && !move.isPending) {
                move.mutate();
              }
            }}
          >
            <label htmlFor={field} className="text-sm">
              New path
            </label>
            <span className="flex items-center gap-1">
              <span className="font-mono text-sm text-slate-500 dark:text-slate-400">
                {ASSETS_PREFIX}
              </span>
              <input
                id={field}
                value={to}
                readOnly={receipt !== null}
                onChange={(e) => {
                  setTo(e.target.value);
                  // The last refusal was about another path.
                  move.reset();
                }}
                className={`${FIELD} min-w-0 flex-1`}
              />
            </span>
            {refusal !== null && (
              <p className="text-sm text-slate-600 dark:text-slate-300">
                {refusal}
              </p>
            )}
            {move.error && (
              <p
                role="alert"
                className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
              >
                {problemDetail(move.error)}
              </p>
            )}
            {receipt !== null && (
              <div role="status" className="flex flex-col gap-1 text-sm">
                <p>{movedText(receipt.linksRewritten)}</p>
                {receipt.note !== null && <p>{receipt.note}</p>}
                {receipt.linksFailed.length > 0 && (
                  <ul className="list-disc pl-5">
                    {receipt.linksFailed.map((page) => (
                      <li key={page.path} className="font-mono">
                        {page.reason !== ""
                          ? `${page.path}: ${page.reason}`
                          : page.path}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
            <div className="flex justify-end gap-2">
              {receipt !== null ? (
                <button
                  type="button"
                  ref={closeButton}
                  onClick={onClose}
                  className={BUTTON.primary}
                >
                  Close
                </button>
              ) : (
                <>
                  <button
                    type="button"
                    onClick={onClose}
                    className={BUTTON.ghost}
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    disabled={move.isPending || !sendable}
                    className={BUTTON.primary}
                  >
                    Move file
                  </button>
                </>
              )}
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
