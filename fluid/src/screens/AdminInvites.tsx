/**
 * Administration > Invites: links that let someone join this instance, each
 * used once.
 *
 * An admin makes a link for any role below admin, with an optional private
 * domain and the level it gives there. The link is shown once, in a field
 * with a copy button, and lives only in this screen's state: Done or leaving
 * the page takes it away for good. Without `service.public_url` the server
 * answers a path under the base path, which this page completes with the
 * address the browser opened it at. The token sits after `#`, so it never
 * reaches a log.
 *
 * The local-account choice is offered only where single sign-on or a proxy
 * names people, because otherwise every link makes a local account anyway.
 * The list says which way in the instance offers.
 *
 * Managers do not use this page: their links come from a domain's Members
 * card.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";

import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import {
  INVITES_KEY,
  createInvite,
  fetchInvites,
  revokeInvite,
} from "../api/invites";
import type { Invite, InviteState, MemberLevel, Role } from "../api/model";
import { useAuth } from "../auth/AuthContext";
import { completeServerLink } from "../base";
import {
  DestructiveAction,
  READ_ONLY_REASON,
} from "../components/DestructiveAction";
import { BUTTON, CONTROL_HEIGHT, FIELD } from "../components/primitives";
import { formatInstant } from "../format";
import { refreshAfterWrite } from "../query/refreshAfterWrite";
import { usePageTitle } from "../usePageTitle";
import type { Notice } from "./accounts";
import { Labelled, NoticeLine, PageHeader, ShownOnce } from "./AdminParts";

/** The roles a link may give an account it makes: every one below admin. */
const ROLES: readonly { value: Role; label: string }[] = [
  { value: "viewer", label: "Viewer" },
  { value: "editor", label: "Editor" },
  { value: "none", label: "None (only invited domains)" },
];

/** The levels a link may give in a domain. */
const LEVELS: readonly { value: MemberLevel; label: string }[] = [
  { value: "viewer", label: "Viewer" },
  { value: "editor", label: "Editor" },
  { value: "manager", label: "Manager" },
];

/** Which links the list shows. */
type Show = "current" | "revoked" | "expired";

const PRIMARY = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.primary} aria-disabled:cursor-default aria-disabled:opacity-50`;
const HINT = "text-xs text-slate-500 dark:text-slate-400";

export default function AdminInvites() {
  usePageTitle("Invites");
  const queryClient = useQueryClient();
  const { capabilities } = useAuth();
  const [notice, setNotice] = useState<Notice | null>(null);
  const [link, setLink] = useState<string | null>(null);
  const [show, setShow] = useState<Show>("current");

  const invites = useQuery({ queryKey: INVITES_KEY, queryFn: fetchInvites });
  const refresh = () =>
    refreshAfterWrite(queryClient, { queryKey: INVITES_KEY });

  function problem(error: Error) {
    setNotice({ kind: "problem", text: problemDetail(error) });
  }

  const revoke = useMutation({
    mutationFn: (invite: Invite) => revokeInvite(invite.id).then(() => invite),
    onSuccess: (invite) => {
      setNotice({ kind: "done", text: `Revoked ${inviteName(invite)}.` });
      void refresh();
    },
    onError: problem,
  });

  const rows = invites.data?.invites ?? [];
  const signIn = invites.data?.sign_in ?? "local";
  const readOnlyReason = capabilities.readOnly ? READ_ONLY_REASON : undefined;

  function list(state: InviteState, title: string, empty: string) {
    return (
      <InviteList
        title={title}
        empty={empty}
        invites={rows.filter((invite) => invite.state === state)}
        revokingId={revoke.isPending ? revoke.variables.id : null}
        readOnlyReason={readOnlyReason}
        onRevoke={(invite) => {
          revoke.mutate(invite);
        }}
      />
    );
  }

  return (
    <div className="flex max-w-4xl flex-col gap-6">
      <PageHeader title="Invites">
        Links that let someone join this instance. Each link works once.
      </PageHeader>

      {notice && <NoticeLine kind={notice.kind} text={notice.text} />}

      {link !== null && (
        <ShownOnce
          key={link}
          label="Invite link"
          copyLabel="Copy the invite link"
          value={link}
          note={
            <p className="text-sm">
              Hand it over yourself. Crystalline sends no mail.
            </p>
          }
          onDone={() => {
            setLink(null);
          }}
        />
      )}

      <CreateInvite
        offerLocal={invites.data !== undefined && signIn !== "local"}
        disabledReason={readOnlyReason}
        onCreated={(made) => {
          setNotice({ kind: "done", text: "Made an invite link." });
          setLink(made);
          void refresh();
        }}
        onProblem={problem}
      />

      <section aria-labelledby="invites-list" className="flex flex-col gap-4">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <h2 id="invites-list" className="text-section">
            Invite links
          </h2>
          <Labelled label="Show">
            {(id) => (
              <select
                id={id}
                value={show}
                onChange={(event) => {
                  setShow(event.target.value as Show);
                }}
                className={FIELD}
              >
                <option value="current">Pending and used</option>
                <option value="revoked">Revoked</option>
                <option value="expired">Expired</option>
              </select>
            )}
          </Labelled>
        </div>
        {invites.error && (
          <NoticeLine kind="problem" text={problemDetail(invites.error)} />
        )}
        {invites.isPending ? (
          <p className="text-sm text-muted">Loading...</p>
        ) : show === "current" ? (
          <>
            {list("pending", "Pending", "No pending links.")}
            {list("used", "Used", "No link has been used yet.")}
          </>
        ) : show === "revoked" ? (
          list("revoked", "Revoked", "No revoked links.")
        ) : (
          list("expired", "Expired", "No expired links.")
        )}
      </section>
    </div>
  );
}

/** The name a link goes by: its label, or its number. */
function inviteName(invite: Invite): string {
  return invite.label ? invite.label : `link ${invite.id}`;
}

/** The form that makes an invite link. */
function CreateInvite({
  offerLocal,
  disabledReason,
  onCreated,
  onProblem,
}: {
  offerLocal: boolean;
  disabledReason: string | undefined;
  onCreated: (link: string) => void;
  onProblem: (error: Error) => void;
}) {
  const ids = useId();
  const [role, setRole] = useState<Role>("viewer");
  const [domain, setDomain] = useState("");
  const [level, setLevel] = useState<MemberLevel>("viewer");
  const [label, setLabel] = useState("");
  const [days, setDays] = useState("7");
  const [local, setLocal] = useState(false);
  const domains = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  const privateDomains = (domains.data?.domains ?? []).filter(
    (summary) => summary.private,
  );
  const create = useMutation({
    // Never retried: a POST that committed and then lost its answer would
    // hold a pending place with a link nobody ever sees.
    retry: false,
    // The link goes to the caller and is not returned, so it never sits in
    // the mutation cache.
    mutationFn: async () => {
      const made = await createInvite({
        role,
        domain: domain === "" ? null : domain,
        level: domain === "" ? null : level,
        label: label.trim() === "" ? null : label.trim(),
        expires_in_days: Number(days),
        local_account: offerLocal && local,
      });
      onCreated(completeServerLink(made.link, made.relative));
      return made.invite.id;
    },
    onSuccess: () => {
      setLabel("");
      setLocal(false);
    },
    onError: onProblem,
  });
  const disabled = disabledReason !== undefined || create.isPending;
  return (
    <section
      aria-labelledby={`${ids}-title`}
      className="flex flex-col gap-3 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <h2 id={`${ids}-title`} className="text-section">
        Make an invite link
      </h2>
      <form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (disabled) return;
          create.mutate();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-3">
          <Labelled label="Role">
            {(id) => (
              <select
                id={id}
                value={role}
                onChange={(event) => {
                  setRole(event.target.value as Role);
                }}
                className={FIELD}
              >
                {ROLES.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            )}
          </Labelled>
          <Labelled label="Domain">
            {(id) => (
              <select
                id={id}
                value={domain}
                onChange={(event) => {
                  setDomain(event.target.value);
                }}
                className={FIELD}
              >
                <option value="">No domain</option>
                {privateDomains.map((summary) => (
                  <option key={summary.name} value={summary.name}>
                    {summary.name}
                  </option>
                ))}
              </select>
            )}
          </Labelled>
          {domain !== "" && (
            <Labelled label={`Level in ${domain}`}>
              {(id) => (
                <select
                  id={id}
                  value={level}
                  onChange={(event) => {
                    setLevel(event.target.value as MemberLevel);
                  }}
                  className={FIELD}
                >
                  {LEVELS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              )}
            </Labelled>
          )}
          <Labelled label="Label">
            {(id) => (
              <input
                id={id}
                autoComplete="off"
                value={label}
                placeholder="Who it is for"
                onChange={(event) => {
                  setLabel(event.target.value);
                }}
                className={FIELD}
              />
            )}
          </Labelled>
          <Labelled label="Expires in days">
            {(id) => (
              <input
                id={id}
                type="number"
                min={1}
                max={30}
                required
                value={days}
                onChange={(event) => {
                  setDays(event.target.value);
                }}
                className={FIELD}
              />
            )}
          </Labelled>
        </div>
        {offerLocal && (
          <div className="flex flex-col gap-0.5 text-sm">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={local}
                aria-describedby={`${ids}-local`}
                onChange={(event) => {
                  setLocal(event.target.checked);
                }}
              />
              Allow a local account with a password
            </label>
            <span id={`${ids}-local`} className={`pl-6 ${HINT}`}>
              Otherwise the person signs in through the identity provider.
            </span>
          </div>
        )}
        <p className={HINT}>
          Only private domains can be invited into. A link lives 1 to 30 days.
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="submit"
            aria-disabled={disabled}
            aria-describedby={
              disabledReason !== undefined ? `${ids}-reason` : undefined
            }
            className={PRIMARY}
          >
            Make link
          </button>
          {disabledReason !== undefined && (
            <span id={`${ids}-reason`} className={HINT}>
              {disabledReason}
            </span>
          )}
        </div>
      </form>
    </section>
  );
}

/** One list of links in one state. */
function InviteList({
  title,
  empty,
  invites,
  revokingId,
  readOnlyReason,
  onRevoke,
}: {
  title: string;
  empty: string;
  invites: Invite[];
  /** The link a revoke is running for, so only its row shows it. */
  revokingId: number | null;
  readOnlyReason: string | undefined;
  onRevoke: (invite: Invite) => void;
}) {
  const ids = useId();
  return (
    <section aria-labelledby={`${ids}-title`} className="flex flex-col gap-2">
      <h3 id={`${ids}-title`} className="text-base font-semibold">
        {title}
      </h3>
      {invites.length === 0 ? (
        <p className="text-sm text-muted">{empty}</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <caption className="sr-only">{title} invite links</caption>
            <thead className={HINT}>
              <tr>
                <th scope="col" className="py-1 pr-3 font-normal">
                  Link
                </th>
                <th scope="col" className="py-1 pr-3 font-normal">
                  Gives
                </th>
                <th scope="col" className="py-1 pr-3 font-normal">
                  Made by
                </th>
                <th scope="col" className="py-1 pr-3 font-normal">
                  {title === "Used" ? "Used" : "Expires"}
                </th>
                <th scope="col" className="py-1 font-normal">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {invites.map((invite) => (
                <tr
                  key={invite.id}
                  className="border-t border-slate-200 align-top dark:border-slate-800"
                >
                  <td className="py-2 pr-3 break-words">
                    {inviteName(invite)}
                  </td>
                  <td className="py-2 pr-3">
                    Role {invite.role}
                    {invite.domain && (
                      <span className="block">
                        {invite.level ?? "viewer"} in {invite.domain}
                      </span>
                    )}
                    {invite.local_account && (
                      <span className={`block ${HINT}`}>Local account</span>
                    )}
                  </td>
                  <td className="py-2 pr-3 break-all">{invite.created_by}</td>
                  <td className="py-2 pr-3">
                    {invite.state === "used" ? (
                      <>
                        {invite.used_at ? formatInstant(invite.used_at) : ""}
                        {invite.used_by && (
                          <span className="block break-all">
                            by {invite.used_by}
                          </span>
                        )}
                      </>
                    ) : (
                      formatInstant(invite.expires_at)
                    )}
                  </td>
                  <td className="py-2">
                    {invite.state === "pending" && (
                      <DestructiveAction
                        label="Revoke"
                        ariaLabel={
                          invite.label
                            ? `Revoke ${invite.label} (link ${invite.id})`
                            : `Revoke link ${invite.id}`
                        }
                        confirmLabel="Confirm revoke"
                        confirmAriaLabel={`Confirm revoke of link ${invite.id}`}
                        pending={revokingId === invite.id}
                        disabledReason={readOnlyReason}
                        onConfirm={() => {
                          onRevoke(invite);
                        }}
                      />
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
