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
 * Three values ask before they change: `sharing: direct` removes the review
 * step for everybody, `read_only: on` refuses every change for everybody, and
 * `reflection: on` changes what `reflections/` is. Each arms the two-step
 * confirmation, and the select goes back to the held value on Keep. While
 * `read_only` is on, its row is the one row that can change: every other
 * select is disabled with the read-only sentence as its description. Either
 * press ends the question, so either press hands the focus back to the
 * select it came from. Every other change posts at once.
 * After a write the answer's `policies` go straight into the manifest
 * query, so the card holds the new values without a second read.
 *
 * A `PolicyKind::Text` row has no select at all: its value is shown, never
 * edited here. `domain_name` changes through a rename, not a PATCH, so its
 * row carries a "Change name" button - gated by the same `own` this card already derives
 * for every select, and disabled with the same reason "Rename domain" on the
 * page itself carries (read-only, or a rename already running). Pressing it
 * does not open a dialog of its own: `onRename` is the screen's own callback,
 * so the one `RenameDomainDialog` mount the page already owns - and the
 * post-rename summary that rides with it - answers both buttons rather than
 * a second dialog answering only this one.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useRef, useState } from "react";

import { syncStatusKey } from "../api/admin";
import { deliverySentence, useAutomation } from "../api/automation";
import type { AutomationResponse } from "../api/automation";
import { problemDetail } from "../api/client";
import {
  domainTreeKey,
  manifestDetailKey,
  manifestKey,
  setDomainPolicies,
} from "../api/domain";
import type { ManifestView, PolicyView } from "../api/domain";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { domainEngramsRoot } from "../api/engrams";
import { fetchMembers, membersKey, sameAccount } from "../api/members";
import {
  REFLECTION_DOMAINS_KEY,
  reflectionStatusKey,
} from "../api/reflectionDomains";
import { useAuth } from "../auth/AuthContext";
import { relativeTime } from "../format";
import { policiesLocked } from "../domainWritable";
import {
  DestructiveAction,
  READ_ONLY_REASON,
  RENAMING_REASON,
} from "./DestructiveAction";
import { BUTTON, FIELD } from "./primitives";
import { refreshAfterWrite } from "../query/refreshAfterWrite";

/** The refusal face, the same one every other screen announces a problem in. */
const ALERT_CLASSES =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

/**
 * The heading's own id, spelled out rather than generated, the way the sync
 * and proposals cards spell theirs: one card per domain page, and a stable
 * name is what a row header and a select are tied together by.
 */
const HEADING_ID = "domain-policies";

/** The values that ask before they post, and what they say. */
const ASKS: Record<
  string,
  { value: string; label: string; say: (branch: string | null) => string }
> = {
  sharing: {
    value: "direct",
    label: "Turn on direct sharing",
    say: (branch) =>
      `Direct sharing removes the review step: every share on this domain commits straight to ${branch ?? "the branch"}, for everybody, until the policy is changed back. Turn it on?`,
  },
  read_only: {
    value: "on",
    label: "Turn on read-only",
    say: () =>
      "Read-only refuses every change to this domain through Crystalline, for everybody, until its owner switches it off. Updates from the team still arrive. Turn it on?",
  },
  reflection: {
    value: "on",
    label: "Turn on the Reflection",
    say: () =>
      "The Reflection keeps pages for people under reflections/ in this domain. Crystalline stops reading that folder as engrams, agents get the Reflection tools for it, and members get a Reflection mode. It waits while engrams still live in reflections/. Turn it on?",
  },
};

/** The trigger rows, and which loop's delivery each one shows. */
const TRIGGERS = [
  ["reflect_trigger", "reflect"],
  ["evolve_trigger", "evolve"],
] as const;

/** What a trigger row says about its webhook, or null when it says nothing. */
function triggerLine(
  key: string,
  automation: AutomationResponse | undefined,
): string | null {
  const loop = TRIGGERS.find(([row]) => row === key)?.[1];
  if (loop === undefined || automation === undefined) {
    return null;
  }
  const delivery = automation[loop]?.delivery;
  return deliverySentence(
    delivery,
    delivery?.last_sent != null
      ? relativeTime(delivery.last_sent, new Date())
      : null,
  );
}

const DRAFT_LINE =
  "Saved to your draft. The domain's policy changes when the draft is shared and lands.";

export function DomainPoliciesCard({
  domain,
  policies,
  branch,
  onRename,
  renaming = false,
}: {
  domain: string;
  policies: PolicyView[];
  /** The branch a direct share would commit onto, or null when none is known. */
  branch: string | null;
  /**
   * Open the rename dialog. The screen owns the one mount of it - and the
   * post-rename summary that rides with it - so this card never mounts a
   * second `RenameDomainDialog` of its own.
   */
  onRename: () => void;
  /**
   * Whether a rename has this domain paused right now, which disables
   * "Change name" with `RENAMING_REASON` the same way the page's own
   * "Rename domain" button is disabled. Defaults to false for a caller that
   * has no rename state to report.
   */
  renaming?: boolean;
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
  const locked = policiesLocked(policies);
  // Asked only while a trigger is on: the server has no delivery to show for
  // a loop whose trigger is off.
  const triggerOn = TRIGGERS.some(([key]) =>
    policies.some((row) => row.key === key && row.effective === "webhook"),
  );
  // The delivery only: no sweep of the domain.
  const automation = useAutomation(domain, { counts: false }, triggerOn).data;
  const [problem, setProblem] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState<Record<string, string>>({});
  // The pending switch that needs a confirmation (direct sharing, read_only
  // or the Reflection): the row whose select shows the asked value while the
  // confirmation is up, or null.
  const [arming, setArming] = useState<string | null>(null);
  const selects = useRef<Record<string, HTMLSelectElement | null>>({});

  const write = useMutation({
    mutationFn: (change: [string, string]) =>
      setDomainPolicies(domain, { [change[0]]: change[1] }),
    onSuccess: (written, [key]) => {
      setProblem((p) => ({ ...p, [key]: "" }));
      setNotice((n) => ({
        ...n,
        [key]: written.draft ? DRAFT_LINE : (written.note ?? ""),
      }));
      // The answer is the manifest as it now reads, so it goes into the query
      // this card's rows come from rather than being read back.
      queryClient.setQueryData<ManifestView>(manifestKey(domain), {
        markdown: written.markdown,
        // A policy is written into a MANIFEST that is there.
        missing: false,
        sections: written.sections,
      });
      // The editor's own copy carries a checksum this write moved, and the
      // status says which way a share goes - which `sharing` just decided.
      void refreshAfterWrite(queryClient, {
        queryKey: manifestDetailKey(domain),
      });
      void refreshAfterWrite(queryClient, { queryKey: syncStatusKey(domain) });
      if (key === "reflection") {
        // Switching the Reflection on or off moves the mode switch on the
        // page above and the Reflection listing every screen reads.
        void refreshAfterWrite(queryClient, {
          queryKey: reflectionStatusKey(domain),
        });
        void refreshAfterWrite(queryClient, {
          queryKey: REFLECTION_DOMAINS_KEY,
        });
      }
      if (key === "read_only") {
        // Every row's `read_only` and every write gate move with the chip,
        // which the manifest write above already moved.
        void refreshAfterWrite(queryClient, { queryKey: DOMAINS_QUERY_KEY });
      }
      if (written.draft) {
        // A draft of the MANIFEST is a file in the caller's own overlay, so
        // the lists that draw the domain's files have moved.
        void refreshAfterWrite(queryClient, {
          queryKey: domainTreeKey(domain),
        });
        void refreshAfterWrite(queryClient, {
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
    if (ASKS[row.key]?.value === value) {
      setArming(row.key);
      return;
    }
    setArming(null);
    write.mutate([row.key, value]);
  }

  return (
    <section aria-labelledby={HEADING_ID} className="flex flex-col gap-4">
      <h2 id={HEADING_ID} className="text-section">
        Domain policies
      </h2>
      <p className="text-sm text-ink-3">
        Every switch this MANIFEST can set. Each one is a line in its
        frontmatter, and the value shown is the one that holds now.
      </p>
      <ul className="flex flex-col text-sm">
        {policies.map((row) => {
          const rowId = `${HEADING_ID}-${row.key}`;
          // A value the frontmatter carries that the registry does not
          // know: the server reads it as the default and says both words,
          // so the row can say what the domain is actually doing. The
          // registry's own values are what decides that, never "declared
          // differs from effective" - a known word the server reads
          // differently is the server's business, not a typo to report.
          const unrecognized =
            row.kind !== "text" &&
            row.declared !== null &&
            !row.values.includes(row.declared);
          const shown =
            arming === row.key
              ? (ASKS[row.key]?.value ?? row.effective)
              : row.effective;
          const lockedOut = locked && row.key !== "read_only";
          const deliveryLine = triggerLine(row.key, automation);
          // The exact reason "Rename domain" is disabled for on the page
          // itself: read-only first, since that holds whatever a rename
          // is doing, then a rename already in flight. The read-only span
          // below is rendered for every row already, so this reuses its id
          // rather than adding a second span with the same words; a rename
          // in flight has no such span yet, so one is added only when this
          // row draws the button that needs it.
          const changeNameDisabledReason = capabilities.readOnly
            ? READ_ONLY_REASON
            : renaming
              ? RENAMING_REASON
              : undefined;
          const changeNameReasonId = capabilities.readOnly
            ? `${rowId}-readonly`
            : `${rowId}-renaming`;
          return (
            // A group named by the key: the name the selects, the output and
            // the specs address a switch by.
            <li
              key={row.key}
              className="border-t border-hair py-4 first:border-t-0 first:pt-0"
            >
              <div
                role="group"
                aria-labelledby={rowId}
                className="grid gap-x-8 gap-y-2 sm:grid-cols-[minmax(0,1fr)_15rem]"
              >
                <div className="min-w-0">
                  <p className="text-ink">{row.meaning}</p>
                  <p className="text-caption mt-1 text-muted">
                    <span id={rowId} className="font-mono">
                      {row.key}
                    </span>
                    {" \u00b7 In the MANIFEST: "}
                    <span>{row.declared ?? "not declared"}</span>
                  </p>
                </div>
                <div className="min-w-0">
                  {row.kind === "text" ? (
                    // A free-text key never changes through this card's own
                    // select, so the value is read-only here whatever
                    // `mayChange` would otherwise say: `domain_name` changes
                    // through the "Change name" button below, which opens the
                    // rename dialog, and `reflection_home` is filled in by the
                    // server when Reflection is switched on.
                    <>
                      <output aria-labelledby={rowId}>
                        {row.effective === "" ? "not set" : row.effective}
                      </output>
                      {row.key === "domain_name" && own && (
                        <>
                          {" "}
                          <button
                            type="button"
                            aria-disabled={
                              changeNameDisabledReason !== undefined
                            }
                            aria-describedby={
                              changeNameDisabledReason !== undefined
                                ? changeNameReasonId
                                : undefined
                            }
                            onClick={() => {
                              if (changeNameDisabledReason !== undefined) {
                                return;
                              }
                              onRename();
                            }}
                            className={`${BUTTON.secondary} aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:aria-disabled:hover:bg-transparent`}
                          >
                            Change name
                          </button>
                          {renaming && !capabilities.readOnly && (
                            <span id={changeNameReasonId} className="sr-only">
                              {RENAMING_REASON}
                            </span>
                          )}
                        </>
                      )}
                    </>
                  ) : mayChange(row) ? (
                    <select
                      ref={(el) => {
                        selects.current[row.key] = el;
                      }}
                      aria-labelledby={rowId}
                      className={`w-full ${FIELD}`}
                      value={shown}
                      disabled={
                        capabilities.readOnly || write.isPending || lockedOut
                      }
                      {...(capabilities.readOnly
                        ? { "aria-describedby": `${rowId}-readonly` }
                        : lockedOut
                          ? { "aria-describedby": `${rowId}-locked` }
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
                  {lockedOut && (
                    <span id={`${rowId}-locked`} className="sr-only">
                      {`'${domain}' is read-only; its owner switches read_only off in the domain policies`}
                    </span>
                  )}
                  {unrecognized && (
                    <p className={`mt-1 ${ALERT_CLASSES}`}>
                      {`read as ${row.effective}; ${row.declared ?? ""} is not a known value`}
                    </p>
                  )}
                  {arming === row.key && ASKS[row.key] !== undefined && (
                    <>
                      {/* What the switch costs, beside the press that makes
                          it: the review step goes for everybody on this
                          domain, not only for whoever is changing it. */}
                      <p className="mt-1 text-sm">
                        {ASKS[row.key]?.say(branch)}
                      </p>
                      <DestructiveAction
                        label={ASKS[row.key]?.label ?? ""}
                        confirmLabel={ASKS[row.key]?.label ?? ""}
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
                          write.mutate([row.key, ASKS[row.key]?.value ?? ""]);
                        }}
                      />
                    </>
                  )}
                  {deliveryLine !== null && (
                    <p className="text-caption mt-1 text-muted">
                      {deliveryLine}
                    </p>
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
                </div>
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
