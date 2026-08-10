/**
 * The guided retirement, and the hard delete folded behind it.
 *
 * A status from the endpoint's own three-word contract, a successor required
 * only for "superseded" (refused by the server otherwise, and prevented here
 * by leaving the field out unless it is), and an optional valid_to bound with
 * the same discipline the frontmatter form's date rows use: an empty field is
 * the unbounded state, and Clear is the only thing that removes a bound
 * already set.
 *
 * The status change is optimistic - the lifecycle banner updates before the
 * round trip answers, per the quality bar for a non-content mutation - and a
 * refusal rolls the cache back and says why in the server's own words.
 *
 * One click further is the delete warning, fed by the graph neighborhood
 * rather than the detail payload's capped inbound sample: `backlinks` is the
 * page's own one-hop read of who points here, which is the actual set within
 * that hop rather than a sample capped at five. A 412 on the delete itself is
 * reported rather than retried - the engram moved on since this dialog read
 * it, and conflict resolution belongs to the editor, not to a delete.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import { engramDetailKey } from "../api/engram";
import type { EngramDetail } from "../api/engram";
import { NEIGHBORHOOD_DEPTH, graphKey } from "../api/graph";
import type { Backlink } from "../api/graph";
import {
  NO_SEARCH,
  SEARCH_DEBOUNCE_MS,
  fetchSearch,
  titleMatchesKey,
} from "../api/search";
import { deleteEngram, retireEngram } from "../api/writes";
import { plural } from "../format";
import { isRetired } from "../lifecycle";
import { domainRoute } from "../paths";
import { RETIREMENT_STATUSES } from "../retirement";
import type { RetireDialogProps } from "./RetireDialog";

const FIELD_CLASSES =
  "w-full rounded border border-slate-300 bg-transparent px-2 py-1 text-sm focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700";

const LABEL_CLASSES =
  "text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400";

const BUTTON_CLASSES =
  "rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800";

/** A successor as the picker holds it: enough to submit and enough to show. */
interface Successor {
  permalink: string;
  title: string;
}

export default function RetireDialogBody({
  engram,
  backlinks,
  onClose,
}: RetireDialogProps): ReactElement {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [status, setStatus] = useState<string>(RETIREMENT_STATUSES[0]);
  const [successor, setSuccessor] = useState<Successor | null>(null);
  const [successorQuery, setSuccessorQuery] = useState("");
  // What the last pause settled on, which is what the server was asked for -
  // the same debounce shape the command palette's title lookup uses.
  const [term, setTerm] = useState("");
  const [validTo, setValidTo] = useState("");
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    const pending = successorQuery.trim();
    if (pending === term) {
      return;
    }
    const timer = setTimeout(() => {
      setTerm(pending);
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [successorQuery, term]);

  const request = {
    ...NO_SEARCH,
    q: term,
    domains: [engram.domain],
    mode: "title" as const,
  };
  const titles = useQuery({
    queryKey: titleMatchesKey(term),
    queryFn: () => fetchSearch(request, 1),
    // Only while a successor is being looked for: no lookup for a status
    // that carries no successor field at all.
    enabled: status === "superseded" && term !== "",
  });
  // Never itself: successor === target is refused server-side, so it is kept
  // out of the picker rather than surfaced only once chosen.
  const options = (titles.data?.hits ?? []).filter(
    (hit) => hit.permalink !== engram.permalink,
  );

  const retire = useMutation({
    mutationFn: () =>
      retireEngram(engram.domain, {
        permalink: engram.permalink,
        status,
        ...(status === "superseded" && successor
          ? { successor: successor.permalink }
          : {}),
        ...(validTo !== "" ? { valid_to: validTo } : {}),
      }),
    onMutate: async () => {
      const key = engramDetailKey(engram.domain, engram.permalink);
      await queryClient.cancelQueries({ queryKey: key });
      const before = queryClient.getQueryData<EngramDetail>(key);
      queryClient.setQueryData<EngramDetail>(key, (old) =>
        old ? { ...old, frontmatter: { ...old.frontmatter, status } } : old,
      );
      return { before };
    },
    onError: (error: Error, _vars, context) => {
      if (context?.before) {
        queryClient.setQueryData(
          engramDetailKey(engram.domain, engram.permalink),
          context.before,
        );
      }
      setProblem(problemDetail(error));
    },
    onSuccess: () => {
      onClose();
    },
    onSettled: () => {
      // The detail is marked stale rather than refetched on the spot: the
      // optimistic status is already what the server just agreed to, and an
      // eager round trip here would only race that value back to what the
      // server held a moment before the mutation answered. A later mount or
      // focus reads it fresh, the same as any other stale query.
      void queryClient.invalidateQueries({
        queryKey: engramDetailKey(engram.domain, engram.permalink),
        refetchType: "none",
      });
      // The neighborhood is different: the retire endpoint wires the
      // supersedes pair itself, so the graph is worth reading again now.
      void queryClient.invalidateQueries({
        queryKey: graphKey(engram.domain, engram.permalink, NEIGHBORHOOD_DEPTH),
      });
    },
  });

  const remove = useMutation({
    mutationFn: () =>
      deleteEngram(engram.domain, engram.permalink, engram.checksum ?? ""),
    onSuccess: () => {
      onClose();
      queryClient.removeQueries({
        queryKey: engramDetailKey(engram.domain, engram.permalink),
      });
      void navigate(domainRoute(engram.domain));
    },
    onError: (error: Error) => {
      // A conflict here is reported, never retried: the engram changed since
      // this dialog read it, and the person re-reads and decides again.
      setProblem(problemDetail(error));
    },
  });

  const blocked = status === "superseded" && !successor;

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
            Retire {engram.title}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            {confirmingDelete
              ? "The record stays only if it is kept. This removes it for good."
              : "Kept for the record rather than as current knowledge, with the successor named where there is one."}
          </Dialog.Description>
          {problem && (
            <p
              role="alert"
              className="mt-3 rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
            >
              {problem}
            </p>
          )}
          {confirmingDelete ? (
            <DeleteWarning
              backlinks={backlinks}
              inboundCount={engram.inboundCount}
              pending={remove.isPending}
              onKeepIt={() => {
                setConfirmingDelete(false);
              }}
              onDelete={() => {
                remove.mutate();
              }}
            />
          ) : (
            <form
              className="mt-3 flex flex-col gap-3"
              onSubmit={(event) => {
                event.preventDefault();
                if (!blocked && !retire.isPending) {
                  setProblem(null);
                  retire.mutate();
                }
              }}
            >
              <fieldset className="flex flex-col gap-1 text-sm">
                <legend className={LABEL_CLASSES}>Status</legend>
                {RETIREMENT_STATUSES.map((value) => (
                  <label key={value} className="flex items-center gap-2">
                    <input
                      type="radio"
                      name="retire-status"
                      checked={status === value}
                      onChange={() => {
                        setStatus(value);
                        if (value !== "superseded") {
                          setSuccessor(null);
                          setSuccessorQuery("");
                        }
                      }}
                    />
                    <span>{value}</span>
                  </label>
                ))}
              </fieldset>
              {status === "superseded" && (
                <div className="flex flex-col gap-1 text-sm">
                  <label className="flex flex-col gap-1">
                    <span className={LABEL_CLASSES}>Successor</span>
                    <input
                      className={FIELD_CLASSES}
                      value={successorQuery}
                      onChange={(event) => {
                        setSuccessorQuery(event.target.value);
                        setSuccessor(null);
                      }}
                      placeholder="Search by title"
                      autoComplete="off"
                    />
                  </label>
                  {term !== "" && !successor && options.length > 0 && (
                    <ul
                      role="listbox"
                      aria-label="Successor matches"
                      className="flex flex-col gap-0.5 rounded border border-slate-200 p-1 dark:border-slate-700"
                    >
                      {options.map((hit) => (
                        <li key={`${hit.domain}/${hit.permalink}`}>
                          <button
                            type="button"
                            role="option"
                            aria-selected={false}
                            className="w-full rounded px-2 py-1 text-left hover:bg-slate-100 dark:hover:bg-slate-800"
                            onClick={() => {
                              setSuccessor({
                                permalink: hit.permalink,
                                title: hit.title,
                              });
                              setSuccessorQuery(hit.title);
                            }}
                          >
                            {hit.title}
                          </button>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              )}
              <div className="flex items-end gap-2">
                <label className="flex flex-col gap-1 text-sm">
                  <span className={LABEL_CLASSES}>Valid to</span>
                  <input
                    type="date"
                    className={FIELD_CLASSES}
                    value={validTo}
                    onChange={(event) => {
                      // Temporal semantics: an empty date IS the unbounded
                      // state, so only a complete date is written here. Clear
                      // below is the only thing that removes a bound already
                      // set - a partial retype must never blank it first.
                      if (event.target.value !== "") {
                        setValidTo(event.target.value);
                      }
                    }}
                  />
                </label>
                {validTo !== "" && (
                  <button
                    type="button"
                    aria-label="Clear valid to"
                    className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
                    onClick={() => {
                      setValidTo("");
                    }}
                  >
                    Clear
                  </button>
                )}
              </div>
              <div className="mt-1 flex flex-wrap items-center justify-between gap-2">
                <button
                  type="button"
                  onClick={() => {
                    setProblem(null);
                    setConfirmingDelete(true);
                  }}
                  className="text-sm text-red-700 underline underline-offset-2 hover:no-underline dark:text-red-400"
                >
                  Delete permanently instead
                </button>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={onClose}
                    className={BUTTON_CLASSES}
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    disabled={blocked || retire.isPending}
                    className={BUTTON_CLASSES}
                  >
                    Retire engram
                  </button>
                </div>
              </div>
            </form>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * The hard-delete warning: who breaks, counted from the graph neighborhood
 * rather than the detail payload's capped inbound sample, and how much more
 * of the index that count could not see.
 */
function DeleteWarning({
  backlinks,
  inboundCount,
  pending,
  onKeepIt,
  onDelete,
}: {
  backlinks: Backlink[];
  inboundCount: number;
  pending: boolean;
  onKeepIt: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="mt-3 flex flex-col gap-3">
      <p className="text-sm">
        {plural(
          backlinks.length,
          "reference into this engram would break",
          "references into this engram would break",
        )}
        :
      </p>
      <ul className="flex flex-col gap-1 text-sm">
        {backlinks.map((backlink) => (
          <li
            key={`${backlink.node.domain}/${backlink.node.permalink}`}
            className={isRetired(backlink.node.status) ? "opacity-60" : ""}
          >
            {backlink.node.title}
          </li>
        ))}
      </ul>
      {inboundCount > backlinks.length && (
        <p className="text-xs text-slate-500 dark:text-slate-400">
          and more across the index: {inboundCount} inbound references counted
        </p>
      )}
      <div className="flex justify-end gap-2">
        <button type="button" onClick={onKeepIt} className={BUTTON_CLASSES}>
          Keep it
        </button>
        <button
          type="button"
          disabled={pending}
          onClick={onDelete}
          className="rounded border border-red-400 bg-red-50 px-3 py-1 text-sm text-red-900 hover:bg-red-100 disabled:opacity-50 dark:border-red-800 dark:bg-red-950 dark:text-red-100"
        >
          Delete permanently
        </button>
      </div>
    </div>
  );
}
