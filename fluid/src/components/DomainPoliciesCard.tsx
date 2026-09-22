/**
 * Every MANIFEST configuration key, as a row the owner can change.
 *
 * The rows come from the server's registry (`sections.policies`), never from
 * a list kept here: a key added to the registry in core shows up with its
 * values, its default and its meaning without a line of this file changing,
 * which is the standing rule.
 *
 * Who may change a key is the server's own verdict read twice: the domain's
 * owner off the members read, which is the derivation `DangerZoneCard` makes
 * for its visibility control, and `canAdminister` for a key the registry
 * marks `admin`. A caller with neither sees the rows and no select. Read-only
 * is not that kind of right: it is a certainty this side already holds, so
 * the select is drawn and disabled with the reason as its accessible
 * description rather than removed - the same trade the danger zone's controls
 * make, and the reason `canWrite` is not consulted here (it folds `readOnly`
 * in, which would take the shut door away instead of showing it).
 *
 * One key asks before it changes: `sharing: direct` removes the review step
 * for everybody, so it arms the two-step confirmation and the select goes
 * back to the held value on Keep with focus back on it. Every other change
 * posts at once. After a write the answer's `policies` go straight into the
 * manifest query, so the card holds the new values without a second read.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useRef, useState } from "react";

import { syncStatusKey } from "../api/admin";
import { problemDetail } from "../api/client";
import {
  domainTreeKey,
  manifestDetailKey,
  manifestKey,
  setDomainPolicies,
} from "../api/domain";
import type { ManifestView, PolicyView } from "../api/domain";
import { domainEngramsRoot } from "../api/engrams";
import { fetchMembers, membersKey, sameAccount } from "../api/members";
import { useAuth } from "../auth/AuthContext";
import { DestructiveAction, READ_ONLY_REASON } from "./DestructiveAction";
import { FIELD } from "./primitives";

/** The refusal face, the same one every other screen announces a problem in. */
const ALERT_CLASSES =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

/**
 * The heading's own id, spelled out rather than generated, the way the sync
 * and proposals cards spell theirs: one card per domain page, and a stable
 * name is what a row header and a select are tied together by.
 */
const HEADING_ID = "domain-policies";

const DRAFT_LINE =
  "Saved to your draft. The domain's policy changes when the draft is shared and lands.";

export function DomainPoliciesCard({
  domain,
  policies,
  branch,
}: {
  domain: string;
  policies: PolicyView[];
  /** The branch a direct share would commit onto, or null when none is known. */
  branch: string | null;
}): ReactElement {
  const { user, capabilities } = useAuth();
  const queryClient = useQueryClient();
  // The members card's own read, under the same key: react-query serves both
  // from one request, and one fact off it is what this card needs.
  const members = useQuery({
    queryKey: membersKey(domain),
    queryFn: () => fetchMembers(domain),
  });
  const owner = members.data?.owner ?? null;
  const own =
    capabilities.canAdminister ||
    (user !== null && owner !== null && sameAccount(owner, user.name));
  const [problem, setProblem] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState<Record<string, string>>({});
  // The pending direct-sharing switch: the row whose select shows `direct`
  // while the confirmation is up, or null.
  const [arming, setArming] = useState<string | null>(null);
  const selects = useRef<Record<string, HTMLSelectElement | null>>({});

  const write = useMutation({
    mutationFn: (change: [string, string]) =>
      setDomainPolicies(domain, { [change[0]]: change[1] }),
    onSuccess: (written, [key]) => {
      setProblem((p) => ({ ...p, [key]: "" }));
      setNotice((n) => ({ ...n, [key]: written.draft ? DRAFT_LINE : "" }));
      // The answer is the manifest as it now reads, so it goes into the query
      // this card's rows come from rather than being read back.
      queryClient.setQueryData<ManifestView>(manifestKey(domain), {
        markdown: written.markdown,
        sections: written.sections,
      });
      // The editor's own copy carries a checksum this write moved, and the
      // status says which way a share goes - which `sharing` just decided.
      void queryClient.invalidateQueries({
        queryKey: manifestDetailKey(domain),
      });
      void queryClient.invalidateQueries({ queryKey: syncStatusKey(domain) });
      if (written.draft) {
        // A draft of the MANIFEST is a file in the caller's own overlay, so
        // the lists that draw the domain's files have moved.
        void queryClient.invalidateQueries({ queryKey: domainTreeKey(domain) });
        void queryClient.invalidateQueries({
          queryKey: domainEngramsRoot(domain),
        });
      }
    },
    onError: (error: Error, [key]) => {
      setProblem((p) => ({ ...p, [key]: problemDetail(error) }));
    },
  });

  /** Whether this caller may change this key at all, refusals aside. */
  function mayChange(row: PolicyView): boolean {
    return row.changedBy === "admin" ? capabilities.canAdminister : own;
  }

  function choose(row: PolicyView, value: string) {
    if (row.key === "sharing" && value === "direct") {
      setArming(row.key);
      return;
    }
    setArming(null);
    write.mutate([row.key, value]);
  }

  return (
    <section
      aria-labelledby={HEADING_ID}
      className="flex flex-col gap-3 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <h2 id={HEADING_ID} className="text-section">
        Domain policies
      </h2>
      <table className="w-full text-sm">
        <caption className="text-caption mb-2 text-left text-slate-500 dark:text-slate-400">
          Every switch this MANIFEST can set
        </caption>
        <thead className="sr-only sm:not-sr-only">
          <tr>
            <th scope="col">Key</th>
            <th scope="col">Declared</th>
            <th scope="col">Effective</th>
            <th scope="col">Meaning</th>
          </tr>
        </thead>
        <tbody>
          {policies.map((row) => {
            const rowId = `${HEADING_ID}-${row.key}`;
            // A value the frontmatter carries that the registry does not
            // know: the server reads it as the default and says both words,
            // so the row can say what the domain is actually doing.
            const unrecognized =
              row.declared !== null && row.declared !== row.effective;
            const shown = arming === row.key ? "direct" : row.effective;
            return (
              // The roles are spelled out because the row stacks below `sm`:
              // a `tr` laid out as a flex column is no longer a row to a
              // screen reader, and its cells are no longer cells.
              <tr
                key={row.key}
                role="row"
                className="flex flex-col gap-1 border-t border-slate-200 py-2 sm:table-row dark:border-slate-800"
              >
                <th
                  id={rowId}
                  role="rowheader"
                  scope="row"
                  className="pr-3 text-left font-mono font-normal"
                >
                  {row.key}
                </th>
                <td role="cell" className="pr-3">
                  {row.declared ?? "not declared"}
                </td>
                <td role="cell" className="pr-3">
                  {mayChange(row) ? (
                    <select
                      ref={(el) => {
                        selects.current[row.key] = el;
                      }}
                      aria-labelledby={rowId}
                      className={FIELD}
                      value={shown}
                      disabled={capabilities.readOnly || write.isPending}
                      {...(capabilities.readOnly
                        ? { "aria-describedby": `${rowId}-readonly` }
                        : {})}
                      onChange={(event) => {
                        choose(row, event.target.value);
                      }}
                    >
                      {row.values.map((value) => (
                        <option key={value} value={value}>
                          {value === row.default ? `${value} (default)` : value}
                        </option>
                      ))}
                    </select>
                  ) : (
                    row.effective
                  )}
                  {capabilities.readOnly && (
                    <span id={`${rowId}-readonly`} className="sr-only">
                      {READ_ONLY_REASON}
                    </span>
                  )}
                  {unrecognized && (
                    <p className={`mt-1 ${ALERT_CLASSES}`}>
                      {`read as ${row.effective}; ${row.declared ?? ""} is not a known value`}
                    </p>
                  )}
                  {arming === row.key && (
                    <>
                      {/* What the switch costs, beside the press that makes
                          it: the review step goes for everybody on this
                          domain, not only for whoever is changing it. */}
                      <p className="mt-1 text-sm">
                        {`Direct sharing removes the review step: every share on this domain commits straight to ${branch ?? "the branch"}, for everybody, until the policy is changed back. Turn it on?`}
                      </p>
                      <DestructiveAction
                        label="Turn on direct sharing"
                        confirmLabel="Turn on direct sharing"
                        pending={write.isPending}
                        // The select is what armed this, so this control
                        // draws no trigger of its own and hands the focus
                        // back to it when the question is given up.
                        hideTrigger
                        confirming
                        onConfirmingChange={(next) => {
                          if (!next) {
                            setArming(null);
                            selects.current[row.key]?.focus();
                          }
                        }}
                        onConfirm={() => {
                          setArming(null);
                          write.mutate([row.key, "direct"]);
                        }}
                      />
                    </>
                  )}
                  {(notice[row.key] ?? "") !== "" && (
                    <p
                      role="status"
                      className="text-caption mt-1 text-slate-500 dark:text-slate-400"
                    >
                      {notice[row.key]}
                    </p>
                  )}
                  {(problem[row.key] ?? "") !== "" && (
                    <p role="alert" className={`mt-1 ${ALERT_CLASSES}`}>
                      {problem[row.key]}
                    </p>
                  )}
                </td>
                <td role="cell" className="text-slate-500 dark:text-slate-400">
                  {row.meaning}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </section>
  );
}
