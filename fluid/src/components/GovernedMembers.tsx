/**
 * The members card's body on a domain GitHub governs: the people GitHub
 * lists on the repository, the grants Crystalline still holds and GitHub
 * overrides, and who may read the domain.
 *
 * Nothing here changes who is a member. GitHub decides that, and the
 * membership routes answer 409 on such a domain, so both tables are
 * read-only. The one choice made here is who may read: everyone signed in,
 * or only the people with access on GitHub. A GitHub admin or maintainer
 * (who holds manager access here) and an instance admin make it; everybody
 * else sees it read-only. Where the environment sets it, it is read-only for
 * everybody, with the sentence that names the variable.
 *
 * Nobody owns a governed domain, so nothing here hands it on.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useId, useState } from "react";

import { problemDetail } from "../api/client";
import { membersKey, setVisibility } from "../api/members";
import type { DomainMember, GithubPerson } from "../api/model";
import { formatDay } from "../format";
import { reflectionWords } from "./reflectionLevels";

const CAPTION = "text-caption font-semibold text-slate-500 dark:text-slate-400";
const MUTED = "text-sm text-slate-500 dark:text-slate-400";

/** The closed reason as a sentence: "Closed. The GitHub App is not installed ...". */
function closedSentence(reason: string): string {
  const trimmed = reason.trim().replace(/\.$/, "");
  return `Closed. ${trimmed.charAt(0).toUpperCase()}${trimmed.slice(1)}.`;
}

/** The archived reason as a sentence that starts with a capital and ends in a dot. */
function archivedSentence(reason: string): string {
  const trimmed = reason.trim().replace(/\.$/, "");
  return `${trimmed.charAt(0).toUpperCase()}${trimmed.slice(1)}.`;
}

export function GovernedMembers({
  domain,
  isPrivate,
  people,
  overridden,
  mayChoose,
  fixedBy,
  closedReason,
  archivedReason,
  disabledReason,
}: {
  domain: string;
  isPrivate: boolean;
  /** The people GitHub lists; null for a caller with no account. */
  people: GithubPerson[] | null;
  overridden: DomainMember[];
  /** Whether this caller may change who reads the domain. */
  mayChoose: boolean;
  /** The sentence that says the environment sets who reads it, or null. */
  fixedBy: string | null;
  closedReason: string | null;
  /** Why the domain is read-only, when its repository is archived; else null. */
  archivedReason: string | null;
  disabledReason?: string | undefined;
}): ReactElement {
  return (
    <>
      {closedReason !== null && (
        <p
          role="status"
          className="rounded bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:bg-amber-950 dark:text-amber-200"
        >
          {closedSentence(closedReason)}
        </p>
      )}
      {archivedReason !== null && (
        <p
          role="status"
          className="rounded bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:bg-amber-950 dark:text-amber-200"
        >
          {archivedSentence(archivedReason)}
        </p>
      )}
      <WhoMayRead
        domain={domain}
        isPrivate={isPrivate}
        mayChoose={mayChoose && fixedBy === null}
        fixedBy={fixedBy}
        disabledReason={disabledReason}
      />
      {people !== null && <PeopleOnGithub people={people} />}
      {overridden.length > 0 && <Overridden rows={overridden} />}
    </>
  );
}

/** The "who may read" switch: everyone signed in, or only GitHub's people. */
function WhoMayRead({
  domain,
  isPrivate,
  mayChoose,
  fixedBy,
  disabledReason,
}: {
  domain: string;
  isPrivate: boolean;
  mayChoose: boolean;
  fixedBy: string | null;
  disabledReason?: string | undefined;
}): ReactElement {
  const queryClient = useQueryClient();
  const name = useId();
  const noteId = useId();
  const [problem, setProblem] = useState<string | null>(null);
  const change = useMutation({
    mutationFn: (makePrivate: boolean) => setVisibility(domain, makePrivate),
    onSuccess: () => {
      setProblem(null);
      void queryClient.invalidateQueries({ queryKey: membersKey(domain) });
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });
  const disabled =
    !mayChoose || disabledReason !== undefined || change.isPending;
  const note = fixedBy ?? disabledReason ?? null;
  const choose = (makePrivate: boolean) => {
    if (disabled || makePrivate === isPrivate) {
      return;
    }
    setProblem(null);
    change.mutate(makePrivate);
  };
  return (
    <fieldset
      className="flex flex-col gap-2"
      aria-describedby={note !== null ? noteId : undefined}
    >
      <legend className="mb-1 text-sm font-medium">Who may read</legend>
      <label className="flex items-center gap-2 text-sm">
        <input
          type="radio"
          name={name}
          checked={!isPrivate}
          disabled={disabled}
          onChange={() => {
            choose(false);
          }}
        />
        Everyone signed in
      </label>
      <label className="flex items-center gap-2 text-sm">
        <input
          type="radio"
          name={name}
          checked={isPrivate}
          disabled={disabled}
          onChange={() => {
            choose(true);
          }}
        />
        Only people with access on GitHub
      </label>
      {note !== null && (
        <p id={noteId} className={MUTED}>
          {note}
        </p>
      )}
      {problem !== null && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problem}
        </p>
      )}
    </fieldset>
  );
}

/** Everyone GitHub lists on the repository, with the level their role gives here. */
function PeopleOnGithub({ people }: { people: GithubPerson[] }): ReactElement {
  const headingId = useId();
  return (
    <div className="flex flex-col gap-2">
      <h3 id={headingId} className="text-sm font-semibold">
        People on GitHub
      </h3>
      {people.length === 0 ? (
        <p className={MUTED}>
          GitHub lists nobody here yet, or has not answered yet.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table
            aria-labelledby={headingId}
            className="w-full text-left text-sm"
          >
            <thead className={CAPTION}>
              <tr>
                <th scope="col" className="px-2 py-2">
                  GitHub
                </th>
                <th scope="col" className="px-2 py-2">
                  Role
                </th>
                <th scope="col" className="px-2 py-2">
                  Level
                </th>
                <th scope="col" className="px-2 py-2">
                  Account
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
              {people.map((person) => (
                <tr key={person.login} className="align-top">
                  <th scope="row" className="px-2 py-2 font-normal">
                    {person.login}
                  </th>
                  <td className="px-2 py-2">{person.github_role}</td>
                  <td className="px-2 py-2">{person.level}</td>
                  <td className="px-2 py-2">
                    {person.account ?? (
                      <span className="text-slate-500 dark:text-slate-400">
                        Not linked
                      </span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/** The membership rows Crystalline still holds, which GitHub overrides. */
function Overridden({ rows }: { rows: DomainMember[] }): ReactElement {
  const headingId = useId();
  return (
    <div className="flex flex-col gap-2">
      <h3 id={headingId} className="text-sm font-semibold">
        Set in Crystalline, overridden by GitHub
      </h3>
      <p className={MUTED}>
        These grants decide nothing while GitHub decides. They are kept in case
        the GitHub App is removed.
      </p>
      <div className="overflow-x-auto">
        <table aria-labelledby={headingId} className="w-full text-left text-sm">
          <thead className={CAPTION}>
            <tr>
              <th scope="col" className="px-2 py-2">
                Account
              </th>
              <th scope="col" className="px-2 py-2">
                Level
              </th>
              <th scope="col" className="px-2 py-2">
                Reflection
              </th>
              <th scope="col" className="px-2 py-2">
                Added
              </th>
            </tr>
          </thead>
          <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
            {rows.map((row) => (
              <tr key={row.principal} className="align-top">
                <th scope="row" className="px-2 py-2 font-normal">
                  {row.principal}
                </th>
                <td className="px-2 py-2">{row.level}</td>
                <td className="px-2 py-2">
                  {reflectionWords(row.level, row.reflection)}
                </td>
                <td className="px-2 py-2 tabular-nums">
                  {formatDay(row.added_at)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
