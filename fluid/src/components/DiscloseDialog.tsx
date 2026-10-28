/**
 * The question a person answers before a section's text from a domain fewer
 * people may read is shared with the page's readers on purpose. The server
 * wrote the question (who reads the page, what becomes readable, that the
 * source's name stays hidden); this dialog shows it as it is and asks for a
 * deliberate yes. Cancel, Escape and a click outside share nothing.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";

import { problemDetail } from "../api/client";
import {
  discloseReflectionSection,
  reflectionReviewKey,
} from "../api/reflection";
import { invalidateReflection } from "../api/reflectionDomains";
import { BUTTON } from "./primitives";

export function DiscloseDialog({
  domain,
  path,
  section,
  question,
  onClose,
}: {
  domain: string;
  path: string;
  section: string;
  question: string;
  onClose: () => void;
}): ReactElement {
  const queryClient = useQueryClient();
  const share = useMutation({
    mutationFn: () => discloseReflectionSection(domain, path, section),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: reflectionReviewKey(domain),
      });
      invalidateReflection(queryClient, domain);
      onClose();
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
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(32rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Share text from a domain fewer people read
          </Dialog.Title>
          <Dialog.Description className="mt-2 text-sm">
            {question}
          </Dialog.Description>
          {share.error && (
            <p
              role="alert"
              className="mt-2 rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
            >
              {problemDetail(share.error)}
            </p>
          )}
          <div className="mt-4 flex justify-end gap-2">
            <button
              type="button"
              autoFocus
              onClick={onClose}
              className={BUTTON.secondary}
            >
              Cancel
            </button>
            <button
              type="button"
              disabled={share.isPending}
              onClick={() => {
                share.mutate();
              }}
              className={BUTTON.destructive}
            >
              Share it
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
