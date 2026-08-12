/**
 * Registering a domain: which of the three kinds it is, and the one or two
 * things that kind needs to be told.
 *
 * The mode decides the form. A local or a virtual domain is a name and nothing
 * else; a team domain is a repository, with the name defaulting to the
 * repository's own and the branch and folder defaulting to the repository's
 * root on its default branch. A field nobody filled in is left OUT of the
 * request rather than sent as an empty string: an absent field is what says
 * "you decide", and an empty one would be this app answering for the server.
 *
 * Team mode is the one that can be impossible from here. Registering against a
 * repository needs the instance's own GitHub credential, so the connection is
 * probed exactly when that mode is chosen, and a disconnected instance is told
 * so with the way to fix it beside the sentence rather than by a submit that
 * fails on the wire.
 *
 * Split from `CreateDomainDialog.tsx` behind a lazy import, for the reason the
 * other dialogs are: the Radix dialog is otherwise not in the entry bundle at
 * all, and every visit would pay for a form only an admin ever opens.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useId, useState } from "react";
import { Link, useNavigate } from "react-router";

import {
  GITHUB_STATUS_KEY,
  createDomain,
  fetchGithubStatus,
} from "../api/admin";
import type { CreateDomainBody, DomainMode } from "../api/admin";
import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { domainRoute, githubSettingsRoute } from "../paths";
import type { CreateDomainDialogProps } from "./CreateDomainDialog";
import { BUTTON, FIELD, FOCUS_RING, Field } from "./primitives";

/** The three kinds of domain, in the order they are worth considering. */
const MODES: { mode: DomainMode; label: string; helper: string }[] = [
  {
    mode: "local",
    label: "Local folder",
    helper: "Markdown files in a folder under the server's own domains root.",
  },
  {
    mode: "virtual",
    label: "Virtual",
    helper: "Engrams live in the server's database, with no files on disk.",
  },
  {
    mode: "github",
    label: "GitHub team",
    helper:
      "Tracks a repository; registering it downloads the shared knowledge.",
  },
];

export default function CreateDomainDialogBody({
  onClose,
}: CreateDomainDialogProps): ReactElement {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const modeGroup = useId();
  const nameField = useId();
  const repoField = useId();
  const branchField = useId();
  const pathField = useId();
  const [mode, setMode] = useState<DomainMode>("local");
  const [name, setName] = useState("");
  const [repo, setRepo] = useState("");
  const [branch, setBranch] = useState("");
  const [path, setPath] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  // Asked only once team mode is on the screen, and cached under the settings
  // screen's own key: an admin who came from there pays nothing for it here.
  const connection = useQuery({
    queryKey: GITHUB_STATUS_KEY,
    queryFn: fetchGithubStatus,
    enabled: mode === "github",
  });
  // Only once an answer is actually in hand. A probe still in flight is not a
  // disconnected instance, and saying so before the server has spoken would
  // put a refusal on screen that may be about to be wrong.
  const disconnected =
    connection.data !== undefined && !connection.data.connected;

  const create = useMutation({
    mutationFn: () => createDomain(requestBody()),
    onSuccess: (created) => {
      // The listing is the sidebar, the home screen and the switcher, all
      // three, and a domain that was just registered is not in the copy any of
      // them are holding.
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
      onClose();
      void navigate(domainRoute(created.domain));
    },
    onError: (error: Error) => {
      // A 409 (already registered) and a 422 (a name the engine will not take)
      // surface verbatim, the way every refusal on this app does.
      setProblem(problemDetail(error));
    },
  });

  /** What goes on the wire: the mode, and only what that mode was told. */
  function requestBody(): CreateDomainBody {
    const named = name.trim();
    if (mode === "github") {
      return {
        mode,
        repo: repo.trim(),
        ...(branch.trim() === "" ? {} : { branch: branch.trim() }),
        ...(path.trim() === "" ? {} : { path: path.trim() }),
        ...(named === "" ? {} : { name: named }),
      };
    }
    return { mode, name: named };
  }

  /** Whether the one field this mode cannot do without has been filled in. */
  const ready = mode === "github" ? repo.trim() !== "" : name.trim() !== "";

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
            New domain
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            What backs it is decided here and stays decided; everything a domain
            holds can move later.
          </Dialog.Description>
          <form
            className="mt-3 flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              if (ready && !disconnected && !create.isPending) {
                setProblem(null);
                create.mutate();
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

            <fieldset className="flex flex-col gap-2 text-sm">
              <legend className="pb-1">Kind</legend>
              {MODES.map((option) => (
                <div key={option.mode} className="flex flex-col">
                  <label className="flex items-center gap-2">
                    <input
                      type="radio"
                      name={modeGroup}
                      value={option.mode}
                      checked={mode === option.mode}
                      aria-describedby={`${modeGroup}-${option.mode}`}
                      onChange={() => {
                        setMode(option.mode);
                      }}
                    />
                    <span>{option.label}</span>
                  </label>
                  {/*
                    Beside the choice rather than inside its name: what a mode
                    is called is two words, and what it means is a sentence, and
                    a radio whose accessible name is both is a radio nobody can
                    ask for.
                  */}
                  <p
                    id={`${modeGroup}-${option.mode}`}
                    className="text-caption pl-6 text-slate-500 dark:text-slate-400"
                  >
                    {option.helper}
                  </p>
                </div>
              ))}
            </fieldset>

            <Field
              id={nameField}
              label="Name"
              helper={
                mode === "github"
                  ? "Optional; the repository's own name when left empty."
                  : "How it is addressed everywhere, this instance over."
              }
            >
              <input
                id={nameField}
                aria-describedby={`${nameField}-help`}
                className={`w-full ${FIELD}`}
                value={name}
                onChange={(event) => {
                  setName(event.target.value);
                }}
                autoFocus
              />
            </Field>

            {mode === "github" && (
              <>
                {disconnected && (
                  <p className="text-sm text-slate-500 dark:text-slate-400">
                    {/*
                      The way out of it, not just the fact. The link is the
                      app's own: accent-700 on this panel is 5.47:1, and
                      accent-400 on the dark panel (slate-900) is 9.59:1.
                    */}
                    <Link
                      to={githubSettingsRoute()}
                      className={`text-accent-700 underline underline-offset-2 hover:no-underline dark:text-accent-400 ${FOCUS_RING}`}
                    >
                      Connect GitHub in settings
                    </Link>{" "}
                    first: this instance has no credential to register a
                    repository with.
                  </p>
                )}
                <Field
                  id={repoField}
                  label="Repository"
                  helper="owner/name, as GitHub writes it."
                >
                  <input
                    id={repoField}
                    aria-describedby={`${repoField}-help`}
                    className={`w-full ${FIELD}`}
                    value={repo}
                    onChange={(event) => {
                      setRepo(event.target.value);
                    }}
                    placeholder="acme/knowledge"
                  />
                </Field>
                <Field
                  id={branchField}
                  label="Branch"
                  helper="Optional; the repository's default branch when left empty."
                >
                  <input
                    id={branchField}
                    aria-describedby={`${branchField}-help`}
                    className={`w-full ${FIELD}`}
                    value={branch}
                    onChange={(event) => {
                      setBranch(event.target.value);
                    }}
                  />
                </Field>
                <Field
                  id={pathField}
                  label="Folder in the repository"
                  helper="Optional; the whole repository when left empty."
                >
                  <input
                    id={pathField}
                    aria-describedby={`${pathField}-help`}
                    className={`w-full ${FIELD}`}
                    value={path}
                    onChange={(event) => {
                      setPath(event.target.value);
                    }}
                  />
                </Field>
              </>
            )}

            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className={BUTTON.secondary}
              >
                Cancel
              </button>
              {/*
                The primary tier, whose disabled face is a filled button gone
                grey rather than an outline at half opacity: a submit that
                cannot run yet reads as waiting rather than as broken.
              */}
              <button
                type="submit"
                disabled={!ready || disconnected || create.isPending}
                className={BUTTON.primary}
              >
                Create domain
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
