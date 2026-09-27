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
 * Team mode also previews the name a create would take: once the repository
 * (and the optional branch and folder) settle for `PEEK_DEBOUNCE_MS`, a peek
 * at `GET /github/domain-name` becomes the Name field's placeholder -
 * `domain_name` when the MANIFEST declares one, else the repository's own
 * name segment, the same fallback an empty submit takes. The peek is read
 * through `useQuery`, keyed by the settled repository, branch and path: a
 * newer edit settles to a different key before an older one's answer can
 * arrive, so whichever response lands, only the CURRENT key's data is ever
 * read - an older, slower answer updates a cache entry nothing here looks at
 * again. Never surfaced as an error and never blocking the form: `retry:
 * false` keeps a 409 (no credential) or a 422 (an unreadable repository) from
 * being retried, and this dialog reads only `.data`, never `.error`, so a
 * failed peek leaves the Name field exactly as unplaceholdered as it is
 * before the debounce ever fires.
 *
 * Split from `CreateDomainDialog.tsx` behind a lazy import, for the reason the
 * other dialogs are: the Radix dialog is otherwise not in the entry bundle at
 * all, and every visit would pay for a form only an admin ever opens.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useEffect, useId, useState } from "react";
import { Link, useNavigate } from "react-router";

import {
  GITHUB_STATUS_KEY,
  createDomain,
  fetchGithubDomainNamePeek,
  fetchGithubStatus,
} from "../api/admin";
import type { CreateDomainBody, DomainMode } from "../api/admin";
import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { useAuth } from "../auth/AuthContext";
import { domainRoute, githubSettingsRoute } from "../paths";
import type { CreateDomainDialogProps } from "./CreateDomainDialog";
import { BUTTON, FIELD, FOCUS_RING, Field } from "./primitives";

/** How long a repository, branch or folder edit waits before it is peeked. */
const PEEK_DEBOUNCE_MS = 400;

/**
 * Whether `value` has the shape a peek is worth asking about: exactly two
 * `owner/name` segments, each made of the characters GitHub allows and
 * neither segment `.` nor `..` - the same shape the server's own
 * `validate_github_repo` requires, so this never asks about a repository the
 * route would refuse before it even reached the forge.
 */
function looksLikeRepo(value: string): boolean {
  const segments = value.split("/");
  if (segments.length !== 2) {
    return false;
  }
  const shape = /^[A-Za-z0-9._-]+$/;
  return segments.every(
    (segment) => shape.test(segment) && segment !== "." && segment !== "..",
  );
}

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
  const { capabilities } = useAuth();
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
  const [private_, setPrivate] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const privateField = useId();

  // Asked only once team mode is on the screen, and cached under the settings
  // screen's own key: an admin who came from there pays nothing for it here.
  const connection = useQuery({
    queryKey: GITHUB_STATUS_KEY,
    queryFn: fetchGithubStatus,
    enabled: mode === "github",
  });

  // What the repository, branch and folder fields last settled to, trimmed:
  // the peek's own input, a beat behind what is actually typed. Debounced
  // rather than fired on every keystroke, the way the search screen's own
  // typing is - see the module doc for why a newer edit's settling is what
  // makes a slower, older peek harmless rather than anything this effect
  // has to cancel itself.
  const [settled, setSettled] = useState({ repo: "", branch: "", path: "" });
  useEffect(() => {
    const timer = setTimeout(() => {
      setSettled({
        repo: repo.trim(),
        branch: branch.trim(),
        path: path.trim(),
      });
    }, PEEK_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [repo, branch, path]);

  const peek = useQuery({
    // A key of its own rather than one under `["settings", "github"]` or
    // `["domains", ...]`: an unrelated invalidation of either of those - the
    // connection settling, a domain registering - must not refetch a peek
    // that has nothing to do with it.
    queryKey: [
      "github-domain-name-peek",
      settled.repo,
      settled.branch,
      settled.path,
    ],
    queryFn: () => fetchGithubDomainNamePeek(settled),
    // Admin only because the route itself is: this dialog is reached by
    // admins alone today, but the check is made here anyway rather than
    // trusted from how the dialog happens to be mounted.
    enabled:
      mode === "github" &&
      capabilities.canAdminister &&
      looksLikeRepo(settled.repo),
    retry: false,
  });
  // Only once the debounce has actually caught up with what is typed: a peek
  // still settling to an EARLIER repository must not flash that repository's
  // name as the placeholder for the one now in the box.
  const peekSettled =
    settled.repo === repo.trim() &&
    settled.branch === branch.trim() &&
    settled.path === path.trim();
  const namePlaceholder =
    mode === "github" && peekSettled && peek.data !== undefined
      ? (peek.data.domainName ?? peek.data.defaultName)
      : undefined;
  // Only in team mode, and only once an answer is actually in hand. A probe
  // still in flight is not a disconnected instance, and saying so before the
  // server has spoken would put a refusal on screen that may be about to be
  // wrong. The mode guard is load bearing beyond the enabled flag above:
  // other screens (the top bar's share readiness probe) fill this cache key,
  // and a cached "disconnected" answer must never gate a local or virtual
  // registration.
  const disconnected =
    mode === "github" &&
    connection.data !== undefined &&
    !connection.data.connected;

  const create = useMutation({
    mutationFn: () => createDomain(requestBody()),
    onSuccess: (created) => {
      // The listing is the sidebar, the home screen and the switcher, all
      // three, and a domain that was just registered is not in the copy any of
      // them are holding. Invalidated and closed either way, so an admin
      // reading the sidebar update never mistakes this for a failed create.
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
      onClose();
      // An empty name means the server named nothing and neither did the
      // request (the reader in api/admin.ts falls back to "" for exactly
      // that case). The domain was still created - the sidebar now shows it -
      // while `domainRoute("")` is a route with an empty segment, which is
      // certainly wrong. Staying put beats navigating to it.
      if (created.domain !== "") {
        void navigate(domainRoute(created.domain));
      }
    },
    onError: (error: Error) => {
      // A 409 (already registered) and a 422 (a name the engine will not take)
      // surface verbatim, the way every refusal on this app does.
      setProblem(problemDetail(error));
    },
  });

  /**
   * What goes on the wire: the mode, and only what that mode was told, plus
   * `private` - which applies to every mode alike, so it rides on both
   * branches rather than only the one somebody happened to be looking at
   * when they checked it. Left out entirely when unchecked, the way every
   * other field nobody filled in is: `false` is the server's own default,
   * and an explicit `false` here would be this app answering for it.
   */
  function requestBody(): CreateDomainBody {
    const named = name.trim();
    if (mode === "github") {
      return {
        mode,
        repo: repo.trim(),
        ...(branch.trim() === "" ? {} : { branch: branch.trim() }),
        ...(path.trim() === "" ? {} : { path: path.trim() }),
        ...(named === "" ? {} : { name: named }),
        ...(private_ ? { private: true } : {}),
      };
    }
    return { mode, name: named, ...(private_ ? { private: true } : {}) };
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
                  ? "Optional; the repository's MANIFEST name when left empty, else its repository name."
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
                placeholder={namePlaceholder}
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

            {/*
              Every mode alike, so it sits below the mode-specific fields
              rather than inside any one of them: a domain is private or it is
              not, whatever backs it.
            */}
            <div className="flex flex-col gap-1 text-sm">
              <label className="flex items-center gap-2">
                <input
                  id={privateField}
                  type="checkbox"
                  checked={private_}
                  aria-describedby={`${privateField}-help`}
                  onChange={(event) => {
                    setPrivate(event.target.checked);
                  }}
                />
                <span>Private</span>
              </label>
              <p
                id={`${privateField}-help`}
                className="text-caption pl-6 text-slate-500 dark:text-slate-400"
              >
                Only you and this instance's admins can reach it until you
                invite somebody else in.
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
