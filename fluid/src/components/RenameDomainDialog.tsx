/**
 * Renaming a domain: its own dialog, reached from the domain page's own
 * button and from the shadowed banner's action.
 *
 * A full rename writes the new name into the MANIFEST and respells every
 * link that named any of this domain's former spellings in the domains the
 * caller can write; `local_only` moves only this machine's own records and
 * leaves the MANIFEST and every link exactly as they are. Neither is a
 * two-step confirm the way unregistering is: nothing is deleted, every
 * former name keeps resolving, and a rename gone wrong is another rename
 * away from fixed.
 *
 * On success the reader is taken to the new address, but not before the
 * cached domain listing already names it: every screen keyed by a domain
 * name - not least this app's own old-address redirect, above every screen
 * under one - reads off that listing. Awaiting an `invalidateQueries` first
 * and navigating after would leave a window where this rename's own listing
 * row is still the stale, pre-rename one; if the new name is the domain's
 * own already-declared canonical name (the "shadowed" case a rename usually
 * exists to fix), that stale row still resolves the new name back to the OLD
 * one, and the redirect sends the reader straight back to the address they
 * just left. So the cache is patched with what this report already says
 * before this component navigates at all, and the navigation itself is a
 * plain, inline call - nothing here waits or defers.
 *
 * The report itself does NOT ride on that navigation's `location.state`,
 * which an earlier version of this file did. The cache patch above notifies
 * its own subscribers - the old-address redirect included - on React Query's
 * own schedule, which is not guaranteed to resolve, as a render, before this
 * component's own navigation call runs right after it; a redirect effect
 * reacting to that patch can still be holding a stale closure over the OLD
 * location (the segment this dialog was opened from) when it fires, and it
 * then issues its own, entirely reasonable, redirect to the very address
 * this component is about to navigate to anyway - carrying whatever state
 * (none) that OLD location held. Both calls target the same underlying
 * history entry, so whichever actually runs last decides what `location.state`
 * survives, and nothing here can promise this component's own call is the
 * one that does: a passive effect and this synchronous callback do not have
 * a fixed relative order in general, only whatever a particular scheduler
 * and a particular router happen to give them today. A `setTimeout` was
 * tried here first, on the theory that deferring this component's own call
 * would let any such effect go first and lose the race for good; it worked,
 * but only because of a specific, undocumented ordering between two
 * unrelated libraries' internal timer scheduling, confirmed by reading
 * their source rather than guaranteed by either one's public contract - a
 * future version of either could reorder it silently. So the report instead
 * waits in the query cache, at `renameReportKey(report.domain)`
 * (`api/admin.ts`), which is not `location.state` and so is not at stake in
 * that race at all: whichever navigation actually lands the reader on this
 * domain's page, `DomainHome` reads the slot for the domain it is now
 * showing and clears it, once, the same page shows regardless of which
 * navigation call put it there. The background `invalidateQueries` that
 * follows is only there to true up anything the optimistic patch could not
 * know (a shadow on the new name, say); by the time it resolves the reader
 * is already on the right address, so the redirect has nothing left to do
 * with it.
 *
 * Not behind a lazy import the way `CreateDomainDialog` is: that split earns
 * its keep on a control mounted on every screen for every session (the
 * frame's own "New domain"), and this one is reached only from a domain page
 * that an owner or an admin is already looking at.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useId, useState } from "react";
import { useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import type { DomainListing } from "../api/domains";
import type { RenameReport } from "../api/admin";
import { renameDomain, renameReportKey } from "../api/admin";
import { domainRoute } from "../paths";
import { BUTTON, FIELD } from "./primitives";

export interface RenameDomainDialogProps {
  domain: string;
  onClose: () => void;
}

export function RenameDomainDialog({
  domain,
  onClose,
}: RenameDomainDialogProps): ReactElement {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const nameField = useId();
  const localOnlyField = useId();
  const [name, setName] = useState("");
  const [localOnly, setLocalOnly] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const rename = useMutation({
    mutationFn: () => renameDomain(domain, name.trim(), localOnly),
    onSuccess: (report: RenameReport) => {
      queryClient.setQueryData(
        DOMAINS_QUERY_KEY,
        (current: DomainListing | undefined) => {
          if (current === undefined) {
            return current;
          }
          return {
            ...current,
            domains: current.domains.map((entry) =>
              entry.name === domain
                ? {
                    ...entry,
                    name: report.domain,
                    canonicalName: report.domain,
                    aliases: report.aliases,
                    renaming: false,
                  }
                : entry,
            ),
          };
        },
      );
      // Set before navigating: whichever navigation actually lands the
      // reader on this domain's page, the report is already waiting there.
      // See the module doc above for why this is not `location.state`.
      queryClient.setQueryData(renameReportKey(report.domain), report);
      onClose();
      void navigate(domainRoute(report.domain), { replace: true });
      // The full truth, in the background, for anything the patch above
      // could not know (a shadow the new name now carries, say): by the
      // time this resolves the reader is already on the right address, so
      // neither this dialog nor the redirect has anything left to do with
      // it.
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
    },
    onError: (error: Error) => {
      // The server's own words, verbatim, the way every refusal on this app
      // surfaces: a 409 (the name is taken, a rename is already running) or a
      // 422 (an invalid name, or - full rename only - an unwritable MANIFEST,
      // whose detail names `--local` / "This machine only" as the way out).
      // Neither ticks the checkbox for the reader: the sentence says the
      // way out, and taking it is a second, deliberate press.
      setProblem(problemDetail(error));
    },
  });

  const ready = name.trim() !== "";

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
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 max-h-[calc(100vh-4rem)] w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Rename domain
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            Every former name this domain answers to keeps resolving, so a link
            spelled the old way still finds it.
          </Dialog.Description>
          <form
            className="mt-3 flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              if (ready && !rename.isPending) {
                setProblem(null);
                rename.mutate();
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

            <div className="flex flex-col gap-1 text-sm">
              <label htmlFor={nameField}>New name</label>
              <input
                id={nameField}
                className={`w-full ${FIELD}`}
                value={name}
                onChange={(event) => {
                  setName(event.target.value);
                }}
                autoFocus
              />
            </div>

            <div className="flex flex-col gap-1 text-sm">
              <label className="flex items-center gap-2">
                <input
                  id={localOnlyField}
                  type="checkbox"
                  checked={localOnly}
                  aria-describedby={`${localOnlyField}-help`}
                  onChange={(event) => {
                    setLocalOnly(event.target.checked);
                  }}
                />
                <span>This machine only</span>
              </label>
              <p
                id={`${localOnlyField}-help`}
                className="text-caption pl-6 text-slate-500 dark:text-slate-400"
              >
                Only this machine&apos;s name changes. The MANIFEST and links in
                other domains stay as they are.
              </p>
            </div>

            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className={BUTTON.secondary}
              >
                Cancel
              </button>
              <button
                type="submit"
                disabled={!ready || rename.isPending}
                className={BUTTON.primary}
              >
                Rename
              </button>
            </div>
          </form>
          {/* Radix's own close affordance is the overlay and Escape; the
              cancel button above is the one this app draws everywhere else,
              so nothing else is added here. */}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
