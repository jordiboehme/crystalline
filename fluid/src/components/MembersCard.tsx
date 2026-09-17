/**
 * Who owns this domain and who is invited into it, while it is private - and
 * the one control that decides whether it is private at all.
 *
 * `GET /domains/{domain}/members` carries no "what may I do here" field, and
 * none is coming: the capability probe idiom this app already uses elsewhere
 * (the share surfaces' `canShare`) is a whole-instance answer, and a private
 * domain's rights are per-domain. So this card derives its own from three
 * things it already has - `owner` and the caller's own row in `members`, both
 * off the one read every render makes anyway, and the admin flag off
 * `/auth/me` - rather than hand-writing a `my_level` shape or waiting for one.
 * Every control it draws from that derivation is still refused by the server
 * if the derivation is ever wrong, and a refusal renders in the server's own
 * words rather than vanishing the control that earned it - this card asks
 * first and shows what happened, it never guesses in place of asking.
 *
 * Nothing here draws for a stranger to a private domain: `require_domain_read`
 * answers a private domain the same 404 an unregistered name gets, so a
 * caller who cannot see it never gets far enough for `DomainHome` to mount
 * this at all. Every caller who does reach it is already the owner, a member
 * at some level, or an instance admin.
 *
 * A domain that is SHARED has no owner and no members to administer - the
 * server's own words, "membership only decides anything while a domain is
 * private" - so the card draws that state plainly instead of an empty table,
 * and offers only the one control a shared domain still has: an admin's way
 * to close it.
 *
 * Three actions here are destructive enough to ask twice, in the confirm
 * pattern this app uses everywhere else (`Profile.tsx`'s `TokenRow`,
 * `DomainHome.tsx`'s own `UnregisterDomain`): removing a member (or leaving,
 * which is the same call naming yourself), handing the domain to somebody
 * else, and closing it. A fourth ships beside them though the brief names
 * only three: re-sharing a domain also throws its membership list away
 * ("making a domain shared again forgets its membership list" - the server's
 * own description), which is exactly the shape of loss the other three ask
 * about, so it gets the same second press.
 *
 * Leaving, or handing the domain away while you were the one holding it, ends
 * this account's own reach into it - the page under this card is about to be
 * a wrong address for whoever just acted, the same way `UnregisterDomain`'s
 * own domain becomes one. Both follow that precedent: invalidate the listing
 * every sidebar and switcher draws from, and leave for `/` rather than sit on
 * a page that is about to refuse to load. Two callers keep the page instead,
 * because neither actually lost anything: an admin who leaves its own
 * membership row still holds `Own` on every domain regardless (`decide`
 * answers `Own` for any admin before it ever looks at the acl), and an owner
 * who "transfers" the domain to itself changed nothing.
 *
 * `capabilities.readOnly` is a certainty this side already holds, unlike a
 * per-domain right - so the five mutations below are disabled rather than
 * offered-then-refused, each with the read-only reason as its own accessible
 * description (`READ_ONLY_REASON`), the same shape `Profile.tsx` gives a
 * verb a read-only instance will not run.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement, ReactNode } from "react";
import { useId, useRef, useState } from "react";
import { useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import {
  fetchMembers,
  membersKey,
  removeMember,
  setMember,
  setOwner,
  setVisibility,
} from "../api/members";
import type { DomainMember, MemberLevel } from "../api/model";
import { useAuth } from "../auth/AuthContext";
import { formatDay } from "../format";
import { BUTTON, FIELD, Field } from "./primitives";

/** Every level a member can be invited at, or moved to. */
const LEVELS: MemberLevel[] = ["viewer", "editor", "manager"];

/**
 * `readOnly` is a certainty this side already holds, straight off the
 * capability probe - unlike a per-domain right, nothing is gained by
 * offering a control the client can prove is refused. The five mutations
 * below are disabled rather than removed, though, and this sentence rides
 * along as each one's accessible description: a door that will not open is
 * still the door, shown as one, not vanished the way a merely *derived*
 * right's absence is.
 */
const READ_ONLY_REASON =
  "This instance is read only, so nothing here can be changed.";

/** Login names are folded to lowercase and trimmed on the way in; compare the same way. */
function sameAccount(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

/** What this caller may do here, derived from the one read every render makes. */
interface MyStanding {
  /** Owner or admin: may transfer, may change visibility either direction. */
  own: boolean;
  /** Manage right or above: may invite, change a level, remove another member. */
  manage: boolean;
  /** This account's own row, when it has one - null for the owner and for an admin with none. */
  mine: DomainMember | null;
}

function myStanding(
  members: DomainMember[],
  owner: string | null,
  isAdmin: boolean,
  myName: string | null,
): MyStanding {
  const mine =
    myName === null
      ? null
      : (members.find((m) => sameAccount(m.principal, myName)) ?? null);
  const own =
    isAdmin ||
    (myName !== null && owner !== null && sameAccount(owner, myName));
  const manage = own || mine?.level === "manager";
  return { own, manage, mine };
}

export function MembersCard({ domain }: { domain: string }) {
  const { user, capabilities } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [problem, setProblem] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const query = useQuery({
    queryKey: membersKey(domain),
    queryFn: () => fetchMembers(domain),
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: membersKey(domain) });

  /** Leaving, or being transferred away from owning it: this account's own reach into the domain just ended. */
  function leftTheDomain() {
    void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
    void navigate("/");
  }

  const invite = useMutation({
    mutationFn: ({
      principal,
      level,
    }: {
      principal: string;
      level: MemberLevel;
    }) => setMember(domain, principal, level),
    onSuccess: () => {
      setProblem(null);
      void invalidate();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const relevel = useMutation({
    mutationFn: ({
      principal,
      level,
    }: {
      principal: string;
      level: MemberLevel;
    }) => setMember(domain, principal, level),
    onSuccess: () => {
      setProblem(null);
      void invalidate();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const remove = useMutation({
    mutationFn: (principal: string) => removeMember(domain, principal),
    onSuccess: (_void, principal) => {
      setProblem(null);
      // An admin keeps `Own` on every domain regardless of its own row
      // (`decide` answers `Own` for any admin before it ever looks at the
      // acl), so an admin leaving its own membership has lost nothing here -
      // exactly the distinction `transfer`'s own success handler already
      // draws below.
      if (
        !capabilities.canAdminister &&
        user !== null &&
        sameAccount(principal, user.name)
      ) {
        leftTheDomain();
        return;
      }
      void invalidate();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const transfer = useMutation({
    mutationFn: (owner: string) => setOwner(domain, owner),
    onSuccess: (_void, newOwner) => {
      setProblem(null);
      const previousOwner = query.data?.owner ?? null;
      const wasOwner =
        !capabilities.canAdminister &&
        user !== null &&
        previousOwner !== null &&
        sameAccount(previousOwner, user.name) &&
        // Transferring to yourself changes nothing - the same account still
        // owns it - so this is not the loss the other branch is for.
        !sameAccount(newOwner, user.name);
      if (wasOwner) {
        leftTheDomain();
        return;
      }
      void invalidate();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const visibility = useMutation({
    mutationFn: (makePrivate: boolean) => setVisibility(domain, makePrivate),
    onSuccess: (_void, makePrivate) => {
      setProblem(null);
      setNotice(
        makePrivate
          ? "This domain is private now."
          : "This domain is shared with everyone again.",
      );
      void invalidate();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  // Drawn only once the read behind it has landed, the way every other card
  // on this page is (`ProposalsCard`, the sync card): a read still in flight
  // is not a state worth a heading, and a read that was refused draws
  // nothing here rather than a second alert stacked under the domain
  // screen's own refusal - a private domain that 404s is already handled by
  // `DomainHome` itself, above this card.
  const data = query.data;
  if (!data) {
    return null;
  }

  // `owner` is optional on the wire (absent means null, exactly as
  // `undefined` and `null` mean the same "no owner" fact everywhere else in
  // this app): normalized once here so every reader below - the standing
  // this card derives, the transfer mutation's own check, the owner row - can
  // stay `string | null` rather than each re-deriving the same fallback.
  const owner = data.owner ?? null;
  const standing = myStanding(
    data.members,
    owner,
    capabilities.canAdminister,
    user?.name ?? null,
  );
  const isPrivate = data.visibility === "private";
  // `readOnly` is a certainty this side already holds, not a derived right -
  // see the constant's own doc for why that earns a different treatment
  // (disabled and explained, never simply offered and left to be refused).
  const disabledReason = capabilities.readOnly ? READ_ONLY_REASON : undefined;

  return (
    <section
      aria-labelledby="domain-members"
      className="flex flex-col gap-4 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <div className="flex flex-wrap items-baseline gap-2">
        <h2 id="domain-members" className="text-section">
          Members
        </h2>
        <span className="text-caption text-slate-500 dark:text-slate-400">
          {isPrivate ? "Private" : "Shared with everyone"}
        </span>
      </div>

      {problem !== null && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problem}
        </p>
      )}
      {notice !== null && (
        <p
          role="status"
          className="rounded bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:bg-slate-900 dark:text-slate-300"
        >
          {notice}
        </p>
      )}

      <VisibilitySection
        isPrivate={isPrivate}
        standing={standing}
        pending={visibility.isPending}
        disabledReason={disabledReason}
        onChange={(makePrivate) => {
          setProblem(null);
          setNotice(null);
          visibility.mutate(makePrivate);
        }}
      />

      {isPrivate ? (
        <>
          <OwnerRow
            owner={owner}
            standing={standing}
            pending={transfer.isPending}
            disabledReason={disabledReason}
            onTransfer={(owner) => {
              setProblem(null);
              setNotice(null);
              transfer.mutate(owner);
            }}
          />
          <MemberTable
            members={data.members}
            myName={user?.name ?? null}
            standing={standing}
            relevelPending={relevel.isPending}
            relevelTarget={relevel.variables?.principal}
            removePending={remove.isPending}
            removeTarget={remove.variables}
            disabledReason={disabledReason}
            onRelevel={(principal, level) => {
              setProblem(null);
              relevel.mutate({ principal, level });
            }}
            onRemove={(principal) => {
              setProblem(null);
              setNotice(null);
              remove.mutate(principal);
            }}
          />
          {standing.manage && (
            <InviteForm
              ownerless={owner === null}
              isAdmin={capabilities.canAdminister}
              pending={invite.isPending}
              disabledReason={disabledReason}
              onInvite={(principal, level) => {
                setProblem(null);
                return invite.mutateAsync({ principal, level });
              }}
            />
          )}
        </>
      ) : (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Every account on this instance can read this domain.
        </p>
      )}
    </section>
  );
}

/**
 * The one control that decides whether this domain is private, in whichever
 * of its two shapes the caller may reach.
 *
 * A manager sees neither direction: deciding who may see a domain at all is
 * not one domain's administration to settle, the same reason it may not
 * transfer ownership. The caption is the spec's own honest disk-truth
 * sentence, shown beside the control rather than always on the card, because
 * it is only worth reading at the moment somebody can act on it.
 */
function VisibilitySection({
  isPrivate,
  standing,
  pending,
  disabledReason,
  onChange,
}: {
  isPrivate: boolean;
  standing: MyStanding;
  pending: boolean;
  disabledReason?: string | undefined;
  onChange: (makePrivate: boolean) => void;
}) {
  // `standing.own` alone, on both directions: closing a shared domain is
  // admin only, and a shared domain never has an owner for the owner clause
  // of `own` to match, so on that side `standing.own` already reduces to
  // exactly "is an admin". Opening a private one is the owner's or an
  // admin's, which is `standing.own` in full.
  if (!standing.own) {
    return null;
  }
  const label = isPrivate ? "Share with everyone" : "Make private";
  return (
    <div className="flex flex-col gap-1">
      <DestructiveAction
        label={label}
        confirmLabel={`Confirm ${label.toLowerCase()}`}
        pending={pending}
        disabledReason={disabledReason}
        onConfirm={() => {
          onChange(!isPrivate);
        }}
      />
      {/*
        The spec's own honest disk-truth sentence, shown whenever a caller
        can act on the private direction: a caller who can only re-share
        already knows a shared domain protects nothing, so the sentence
        earns its place beside the control that would close one instead.
      */}
      <p className="text-caption text-slate-500 dark:text-slate-400">
        {isPrivate
          ? "Opening this domain forgets who was invited into it."
          : "Private domains protect from other users of this instance, not from whoever operates the machine."}
      </p>
    </div>
  );
}

/** The account that owns this domain, honestly - including when there is none. */
function OwnerRow({
  owner,
  standing,
  pending,
  disabledReason,
  onTransfer,
}: {
  owner: string | null;
  standing: MyStanding;
  pending: boolean;
  disabledReason?: string | undefined;
  onTransfer: (owner: string) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      <span className="font-medium">Owner</span>
      {owner === null ? (
        <span className="text-slate-500 dark:text-slate-400">
          No owner - removed from the account roster.
        </span>
      ) : (
        <span>{owner}</span>
      )}
      {/*
        No `owner !== null` clause: `set_owner` needs only `DomainRight::Own`
        and a private domain - it never inspects the current owner - and on
        an ownerless domain `standing.own` already reduces to exactly "is an
        admin", the owner clause of `own` having nothing to match. So this is
        already admin-or-owner-only without the extra guard, and dropping it
        is what gives an ownerless private domain a path back to having one.
      */}
      {standing.own && (
        <TransferOwnership
          pending={pending}
          disabledReason={disabledReason}
          onTransfer={onTransfer}
        />
      )}
    </div>
  );
}

/** Hand the domain to a different account, by typing its login name and confirming. */
function TransferOwnership({
  pending,
  disabledReason,
  onTransfer,
}: {
  pending: boolean;
  disabledReason?: string | undefined;
  onTransfer: (owner: string) => void;
}) {
  const fieldId = useId();
  return (
    <DestructiveAction
      label="Transfer ownership"
      confirmLabel="Confirm transfer"
      pending={pending}
      disabledReason={disabledReason}
      requireValue
      onConfirm={(value) => {
        onTransfer(value.trim());
      }}
    >
      {(value, setValue) => (
        <>
          <label htmlFor={fieldId} className="sr-only">
            New owner
          </label>
          <input
            id={fieldId}
            autoFocus
            required
            autoComplete="off"
            placeholder="login name"
            value={value}
            onChange={(event) => {
              setValue(event.target.value);
            }}
            className={`w-40 ${FIELD}`}
          />
        </>
      )}
    </DestructiveAction>
  );
}

/** Every membership row: who, at what level, and what this caller may do about it. */
function MemberTable({
  members,
  myName,
  standing,
  relevelPending,
  relevelTarget,
  removePending,
  removeTarget,
  disabledReason,
  onRelevel,
  onRemove,
}: {
  members: DomainMember[];
  myName: string | null;
  standing: MyStanding;
  relevelPending: boolean;
  relevelTarget: string | undefined;
  removePending: boolean;
  removeTarget: string | undefined;
  disabledReason?: string | undefined;
  onRelevel: (principal: string, level: MemberLevel) => void;
  onRemove: (principal: string) => void;
}) {
  if (members.length === 0) {
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        Nobody else is invited into this domain yet.
      </p>
    );
  }
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <caption className="sr-only">Members</caption>
        <thead className="text-caption font-semibold text-slate-500 dark:text-slate-400">
          <tr>
            <th scope="col" className="px-2 py-2">
              Account
            </th>
            <th scope="col" className="px-2 py-2">
              Level
            </th>
            <th scope="col" className="px-2 py-2">
              Added
            </th>
            <th scope="col" className="px-2 py-2">
              Actions
            </th>
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
          {members.map((member) => {
            const isSelf =
              myName !== null && sameAccount(member.principal, myName);
            const mayRemove = standing.manage || isSelf;
            return (
              <MemberRow
                key={member.principal}
                member={member}
                isSelf={isSelf}
                mayRelevel={standing.manage}
                mayRemove={mayRemove}
                relevelPending={
                  relevelPending && relevelTarget === member.principal
                }
                removePending={
                  removePending && removeTarget === member.principal
                }
                disabledReason={disabledReason}
                onRelevel={(level) => {
                  onRelevel(member.principal, level);
                }}
                onRemove={() => {
                  onRemove(member.principal);
                }}
              />
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function MemberRow({
  member,
  isSelf,
  mayRelevel,
  mayRemove,
  relevelPending,
  removePending,
  disabledReason,
  onRelevel,
  onRemove,
}: {
  member: DomainMember;
  isSelf: boolean;
  mayRelevel: boolean;
  mayRemove: boolean;
  relevelPending: boolean;
  removePending: boolean;
  disabledReason?: string | undefined;
  onRelevel: (level: MemberLevel) => void;
  onRemove: () => void;
}) {
  const selectId = useId();
  const reasonId = useId();
  const removeLabel = isSelf ? "Leave" : "Remove";
  return (
    <tr className="align-top">
      <th scope="row" className="px-2 py-2 font-normal">
        {member.principal}
        {isSelf && (
          <span className="text-caption text-slate-500 dark:text-slate-400">
            {" (you)"}
          </span>
        )}
      </th>
      <td className="px-2 py-2">
        {mayRelevel ? (
          <>
            <label htmlFor={selectId} className="sr-only">
              {`Level for ${member.principal}`}
            </label>
            <select
              id={selectId}
              value={member.level}
              disabled={relevelPending}
              // `aria-disabled`, not `disabled`: a read-only instance is a
              // certainty rather than a passing loading state (`relevelPending`
              // stays native), and a select taken fully out of the tab order
              // could never announce the reason it carries. See
              // `Layout.tsx`'s own `ShareChanges` for the same call.
              aria-disabled={disabledReason !== undefined}
              aria-describedby={
                disabledReason !== undefined ? reasonId : undefined
              }
              onChange={(event) => {
                if (disabledReason !== undefined) {
                  return;
                }
                onRelevel(event.target.value as MemberLevel);
              }}
              className={`${FIELD} aria-disabled:cursor-default aria-disabled:opacity-50`}
            >
              {LEVELS.map((level) => (
                <option key={level} value={level}>
                  {level}
                </option>
              ))}
            </select>
            {disabledReason !== undefined && (
              <span id={reasonId} className="sr-only">
                {disabledReason}
              </span>
            )}
          </>
        ) : (
          member.level
        )}
      </td>
      <td className="px-2 py-2 tabular-nums">{formatDay(member.added_at)}</td>
      <td className="px-2 py-2">
        {mayRemove && (
          <DestructiveAction
            label={removeLabel}
            confirmLabel={`Confirm ${removeLabel.toLowerCase()}`}
            ariaLabel={`${removeLabel} ${member.principal}`}
            confirmAriaLabel={`Confirm ${removeLabel.toLowerCase()} ${member.principal}`}
            pending={removePending}
            disabledReason={disabledReason}
            onConfirm={onRemove}
          />
        )}
      </td>
    </tr>
  );
}

/** Invite an account at a level, or - on an ownerless domain, for an admin - assign it a manager. */
function InviteForm({
  ownerless,
  isAdmin,
  pending,
  disabledReason,
  onInvite,
}: {
  ownerless: boolean;
  isAdmin: boolean;
  pending: boolean;
  disabledReason?: string | undefined;
  /**
   * Returns the request's own promise rather than firing and forgetting: the
   * field below clears once this resolves, not once it is merely sent, so a
   * refusal leaves the typed name on screen to be fixed instead of thrown
   * away.
   */
  onInvite: (principal: string, level: MemberLevel) => Promise<unknown>;
}) {
  const assigning = ownerless && isAdmin;
  const [principal, setPrincipal] = useState("");
  const [level, setLevel] = useState<MemberLevel>(
    assigning ? "manager" : "viewer",
  );
  const nameField = useId();
  const levelField = useId();
  const reasonId = useId();
  const disabled = pending || disabledReason !== undefined;

  return (
    <form
      className="flex flex-wrap items-end gap-3 border-t border-slate-200 pt-4 dark:border-slate-800"
      onSubmit={(event) => {
        event.preventDefault();
        const trimmed = principal.trim();
        if (trimmed === "" || disabled) {
          return;
        }
        onInvite(trimmed, level)
          .then(() => {
            setPrincipal("");
          })
          .catch(() => {
            // Refused: the card above already shows the server's own words,
            // and the typed name stays so it can be fixed rather than
            // retyped from scratch.
          });
      }}
    >
      {assigning && (
        <p className="w-full text-sm text-slate-500 dark:text-slate-400">
          This domain has no owner. Invite someone as manager to administer it,
          or transfer ownership to assign one directly.
        </p>
      )}
      <Field id={nameField} label="Account">
        <input
          id={nameField}
          required
          autoComplete="off"
          placeholder="login name"
          value={principal}
          // Read-only rather than disabled: there is no press to guard here,
          // and a native `readOnly` input refuses edits on its own while
          // staying in the tab order and announcing the reason - unlike
          // `disabled`, which would also take it out of the tab order for no
          // reason this field needs.
          readOnly={disabledReason !== undefined}
          aria-describedby={disabledReason !== undefined ? reasonId : undefined}
          onChange={(event) => {
            setPrincipal(event.target.value);
          }}
          className={`w-40 ${FIELD} read-only:cursor-default read-only:opacity-50`}
        />
      </Field>
      <Field id={levelField} label="Level">
        <select
          id={levelField}
          value={level}
          aria-disabled={disabledReason !== undefined}
          aria-describedby={disabledReason !== undefined ? reasonId : undefined}
          onChange={(event) => {
            if (disabledReason !== undefined) {
              return;
            }
            setLevel(event.target.value as MemberLevel);
          }}
          className={`${FIELD} aria-disabled:cursor-default aria-disabled:opacity-50`}
        >
          {LEVELS.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      </Field>
      <button
        type="submit"
        // `aria-disabled`, not `disabled`: `onSubmit` above already guards
        // the press (`if (trimmed === "" || disabled) return`), so the only
        // thing a native `disabled` attribute would add here is removing the
        // button from the tab order and silencing the reason it carries.
        aria-disabled={disabled}
        aria-describedby={disabledReason !== undefined ? reasonId : undefined}
        className={`${BUTTON.primary} aria-disabled:bg-slate-200 aria-disabled:text-slate-500 dark:aria-disabled:bg-slate-800 dark:aria-disabled:text-slate-500`}
      >
        {assigning ? "Assign manager" : "Invite"}
      </button>
      {disabledReason !== undefined && (
        <span id={reasonId} className="sr-only">
          {disabledReason}
        </span>
      )}
    </form>
  );
}

/**
 * The two-step confirm this app uses for every destructive control: a trigger
 * that asks, a second press that means it, focus handed back to the trigger
 * on Escape or on losing the confirm without pressing it.
 *
 * The pattern this repeats to the letter is `Profile.tsx`'s `TokenRow` and
 * `DomainHome.tsx`'s own `UnregisterDomain` - a dialog the browser owns
 * cannot be reached by a test, styled, or dismissed by keyboard the way this
 * can. `TransferOwnership` is built on this one rather than hand-rolling the
 * same machinery a third time in this file: `children` is the one thing it
 * needs beyond a plain confirm, an extra control rendered between trigger and
 * confirm that carries its own value through to `onConfirm`.
 */
function DestructiveAction({
  label,
  confirmLabel,
  ariaLabel,
  confirmAriaLabel,
  pending,
  disabledReason,
  requireValue = false,
  children,
  onConfirm,
}: {
  label: string;
  confirmLabel: string;
  /**
   * The trigger's accessible name, when the visible `label` alone would not
   * be unique on the page - a row's "Remove" beside every other row's own.
   * Defaults to `label`.
   */
  ariaLabel?: string;
  /** Same reason, for the confirm press. Defaults to `confirmLabel`. */
  confirmAriaLabel?: string;
  pending: boolean;
  /**
   * When set, the trigger is disabled and this is exposed as its accessible
   * description - a certainty already held, not a right this side is merely
   * guessing at, so the door is shown shut rather than removed.
   */
  disabledReason?: string | undefined;
  /** Whether the confirm press needs a non-empty `value` before it may fire. */
  requireValue?: boolean;
  /** An extra control rendered between trigger and confirm, e.g. a text field. */
  children?: (value: string, setValue: (value: string) => void) => ReactNode;
  onConfirm: (value: string) => void;
}): ReactElement {
  const [confirming, setConfirming] = useState(false);
  const [value, setValue] = useState("");
  const trigger = useRef<HTMLButtonElement>(null);
  const name = ariaLabel ?? label;
  const confirmName = confirmAriaLabel ?? confirmLabel;
  const reasonId = useId();
  // `aria-disabled`, not `disabled`, on both buttons below: `disabled` takes
  // a control out of the tab order entirely, so a keyboard user could never
  // land on it, let alone hear the reason `aria-describedby` attaches to it.
  // This repo already ruled on exactly this trade in `Layout.tsx`'s own
  // `ShareChanges` - reachable by pointer and by keyboard, says it will not
  // act, and does not, because the press is guarded as well as the face.
  const disabled = pending || disabledReason !== undefined;
  const confirmDisabled =
    pending ||
    disabledReason !== undefined ||
    (requireValue && value.trim() === "");

  function abandon() {
    setConfirming(false);
    setValue("");
    trigger.current?.focus();
  }

  return (
    <span
      className="inline-flex flex-wrap items-center gap-2"
      onKeyDown={(event) => {
        if (event.key === "Escape" && confirming) {
          event.stopPropagation();
          abandon();
        }
      }}
      onBlur={(event) => {
        const next = event.relatedTarget;
        if (
          confirming &&
          next instanceof Node &&
          !event.currentTarget.contains(next)
        ) {
          setConfirming(false);
        }
      }}
    >
      <button
        ref={trigger}
        type="button"
        aria-label={name}
        aria-expanded={confirming}
        aria-describedby={disabledReason !== undefined ? reasonId : undefined}
        aria-disabled={disabled}
        onClick={() => {
          if (disabled) {
            return;
          }
          setConfirming(true);
        }}
        className={`${BUTTON.destructive} aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:aria-disabled:hover:bg-transparent`}
      >
        {label}
      </button>
      {disabledReason !== undefined && (
        <span id={reasonId} className="sr-only">
          {disabledReason}
        </span>
      )}
      {confirming && (
        <>
          {children?.(value, setValue)}
          <button
            type="button"
            autoFocus={children === undefined}
            aria-label={confirmName}
            aria-describedby={
              disabledReason !== undefined ? reasonId : undefined
            }
            aria-disabled={confirmDisabled}
            onClick={() => {
              if (confirmDisabled) {
                return;
              }
              setConfirming(false);
              const confirmed = value;
              setValue("");
              onConfirm(confirmed);
            }}
            className={`${BUTTON.destructive} aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:aria-disabled:hover:bg-transparent`}
          >
            {confirmLabel}
          </button>
          <button type="button" onClick={abandon} className={BUTTON.secondary}>
            Keep
          </button>
        </>
      )}
    </span>
  );
}
