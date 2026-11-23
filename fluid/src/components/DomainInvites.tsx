/**
 * Invite links for one private domain, in its Members card: for someone who
 * has no account here yet.
 *
 * Drawn only where the members read says `can_invite`: an admin, or the
 * owner or a manager while the instance lets them make links, on a private
 * domain GitHub does not govern. A link from here is a link to this domain:
 * it sends instance role `none` for every maker, admins included, so the
 * person sees only the domains they are invited into, and membership here at
 * the level picked (the server keeps that level within the maker's own). An
 * admin who wants a wider role makes the link in Administration > Invites.
 *
 * The made link is shown once, with a copy button, and lives only in this
 * component's state. The list under it is the caller's own pending links to
 * this domain, each with a revoke behind the two-step confirm. An admin's
 * other links live in Administration > Invites.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";

import { problemDetail } from "../api/client";
import {
  INVITES_KEY,
  createInvite,
  fetchInvites,
  revokeInvite,
} from "../api/invites";
import { sameAccount } from "../api/members";
import type { Invite, MemberLevel } from "../api/model";
import { completeServerLink } from "../base";
import { formatInstant } from "../format";
import { refreshAfterWrite } from "../query/refreshAfterWrite";
import { ShownOnce } from "../screens/AdminParts";
import { DestructiveAction } from "./DestructiveAction";
import { BUTTON, CONTROL_HEIGHT, FIELD } from "./primitives";

/** The levels a link may give here. */
const LEVELS: readonly { value: MemberLevel; label: string }[] = [
  { value: "viewer", label: "Viewer" },
  { value: "editor", label: "Editor" },
  { value: "manager", label: "Manager" },
];

const HINT = "text-xs text-slate-500 dark:text-slate-400";
const PRIMARY = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.primary} aria-disabled:cursor-default aria-disabled:opacity-50`;
const SECONDARY = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.secondary}`;

/** The name a link goes by: its label, or its number. */
function inviteName(invite: Invite): string {
  return invite.label ? invite.label : `link ${invite.id}`;
}

export function DomainInvites({
  domain,
  myName,
  formOpen,
  onFormOpenChange,
  disabledReason,
}: {
  domain: string;
  myName: string | null;
  /** Whether the make form shows; the picker's "Invite someone new" opens it too. */
  formOpen: boolean;
  onFormOpenChange: (open: boolean) => void;
  disabledReason?: string | undefined;
}) {
  const ids = useId();
  const queryClient = useQueryClient();
  const [link, setLink] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [level, setLevel] = useState<MemberLevel>("viewer");
  const [label, setLabel] = useState("");
  const [days, setDays] = useState("7");

  const invites = useQuery({ queryKey: INVITES_KEY, queryFn: fetchInvites });
  const refresh = () =>
    refreshAfterWrite(queryClient, { queryKey: INVITES_KEY });

  const create = useMutation({
    // Never retried: a POST that committed and lost its answer would hold a
    // pending place with a link nobody sees.
    retry: false,
    // The link goes to state here and is not returned, so it never sits in
    // the mutation cache.
    mutationFn: async () => {
      const made = await createInvite({
        role: "none",
        domain,
        level,
        label: label.trim() === "" ? null : label.trim(),
        expires_in_days: Number(days),
      });
      setLink(completeServerLink(made.link, made.relative));
      return made.invite.id;
    },
    onSuccess: () => {
      setProblem(null);
      setLabel("");
      onFormOpenChange(false);
      void refresh();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const revoke = useMutation({
    mutationFn: (invite: Invite) => revokeInvite(invite.id),
    onSuccess: () => {
      setProblem(null);
      void refresh();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const mine = (invites.data?.invites ?? []).filter(
    (invite) =>
      invite.state === "pending" &&
      invite.domain === domain &&
      myName !== null &&
      sameAccount(invite.created_by, myName),
  );
  const disabled = disabledReason !== undefined || create.isPending;

  return (
    <section
      aria-labelledby={`${ids}-title`}
      className="flex flex-col gap-3 border-t border-slate-200 pt-4 dark:border-slate-800"
    >
      <h3 id={`${ids}-title`} className="text-base font-semibold">
        Invite links
      </h3>
      <p className="text-sm text-ink-3">
        For someone without an account here. A link works once. The person sees
        only the domains they are invited into.
      </p>
      {problem !== null && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problem}
        </p>
      )}
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
      {formOpen ? (
        <form
          aria-label="Make an invite link"
          className="flex flex-wrap items-end gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (disabled) return;
            create.mutate();
          }}
        >
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={`${ids}-access`}>{`Access in ${domain}`}</label>
            <select
              id={`${ids}-access`}
              // The form opens on a press, so the focus follows it here.
              autoFocus
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
          </div>
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={`${ids}-label`}>Label</label>
            <input
              id={`${ids}-label`}
              autoComplete="off"
              placeholder="Who it is for"
              value={label}
              onChange={(event) => {
                setLabel(event.target.value);
              }}
              className={`w-40 ${FIELD}`}
            />
          </div>
          <div className="flex flex-col gap-1 text-sm">
            <label htmlFor={`${ids}-days`}>Expires in days</label>
            <input
              id={`${ids}-days`}
              type="number"
              min={1}
              max={30}
              required
              value={days}
              onChange={(event) => {
                setDays(event.target.value);
              }}
              className={`w-24 ${FIELD}`}
            />
          </div>
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
          <button
            type="button"
            onClick={() => {
              onFormOpenChange(false);
            }}
            className={SECONDARY}
          >
            Cancel
          </button>
          {disabledReason !== undefined && (
            <span id={`${ids}-reason`} className={HINT}>
              {disabledReason}
            </span>
          )}
        </form>
      ) : (
        <div>
          <button
            type="button"
            onClick={() => {
              onFormOpenChange(true);
            }}
            className={SECONDARY}
          >
            Make an invite link
          </button>
        </div>
      )}
      {mine.length > 0 && (
        <ul aria-label="Your pending links" className="flex flex-col gap-2">
          {mine.map((invite) => (
            <li
              key={invite.id}
              className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm"
            >
              <span className="font-medium break-words">
                {inviteName(invite)}
              </span>
              <span>{invite.level ?? "viewer"}</span>
              <span className={HINT}>
                {`Expires ${formatInstant(invite.expires_at)}`}
              </span>
              <DestructiveAction
                label="Revoke"
                ariaLabel={
                  invite.label
                    ? `Revoke ${invite.label} (link ${String(invite.id)})`
                    : `Revoke link ${String(invite.id)}`
                }
                confirmLabel="Confirm revoke"
                confirmAriaLabel={`Confirm revoke of link ${String(invite.id)}`}
                pending={revoke.isPending && revoke.variables.id === invite.id}
                disabledReason={disabledReason}
                onConfirm={() => {
                  revoke.mutate(invite);
                }}
              />
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
