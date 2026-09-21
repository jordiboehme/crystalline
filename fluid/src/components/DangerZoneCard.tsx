/**
 * The two ways of taking a domain away from the people who read it:
 * unregistering it, and changing who may see it at all.
 *
 * They sit together, in a box that wears the destructive button's own red
 * border, because what they have in common is what a reader needs warning
 * about: neither is undone by pressing something else afterwards.
 * Unregistering ends this instance's reach into a folder, or - on a virtual
 * domain, whose engrams are the database's - deletes them outright. Closing a
 * shared domain hides it from everybody who was reading it, and opening a
 * private one forgets the membership list on the way out.
 *
 * So both ask for the domain's name to be typed rather than for a second
 * press alone, which is what this app asks for a loss it can describe in one
 * sentence. The visibility control moved here from the members card for
 * exactly that reason: administering a team is one thing, and deciding who
 * may see everything in a domain is another.
 *
 * The whole card is drawn under `canAdminister` by the screen that mounts it,
 * which is the right both verbs need: unregistering is admin-only, making a
 * domain private is admin-only, and re-sharing one needs `Own`, which an
 * admin holds on every domain before the acl is ever read.
 *
 * `confirming` is the screen's rather than the control's, because the command
 * palette offers the same unregister row: the keyboard route arms this exact
 * confirmation - and scrolls it into view, since it sits at the foot of a
 * long page - rather than skipping the step the control exists for.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useEffect, useId, useRef, useState } from "react";
import { useNavigate } from "react-router";

import { unregisterDomain } from "../api/admin";
import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { fetchMembers, membersKey, setVisibility } from "../api/members";
import { useAuth } from "../auth/AuthContext";
import { DestructiveAction, READ_ONLY_REASON } from "./DestructiveAction";

export function DangerZoneCard({
  domain,
  kind,
  confirming,
  onConfirmingChange,
}: {
  domain: string;
  /** `file`, `virtual`, or null when the listing did not say. */
  kind: string | null;
  /** Whether the unregister confirmation is armed; the palette arms it too. */
  confirming: boolean;
  onConfirmingChange: (confirming: boolean) => void;
}): ReactElement {
  const { capabilities } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const headingId = useId();
  const [problem, setProblem] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const card = useRef<HTMLElement>(null);

  // The same read the members card makes, under the same key: react-query
  // serves both from one request, and this card needs only one fact off it -
  // which way round the visibility control points.
  const members = useQuery({
    queryKey: membersKey(domain),
    queryFn: () => fetchMembers(domain),
  });

  const unregister = useMutation({
    // A virtual domain's engrams are deleted with it and the server refuses to
    // guess that the loss was intended, so the confirmed press is what carries
    // `purge`: the typed name is the confirmation the flag stands for.
    mutationFn: () => unregisterDomain(domain, kind === "virtual"),
    onSuccess: () => {
      // The listing is what every sidebar, card and switcher draws from, and
      // the domain this screen is about is no longer in it.
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
      // Nowhere to stay: this address is now a wrong address.
      void navigate("/");
    },
    onError: (error: Error) => {
      onConfirmingChange(false);
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
      // The members card reads the same key and says which state the domain
      // is in beside its own heading, so it hears about this at once.
      void queryClient.invalidateQueries({ queryKey: membersKey(domain) });
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  // Armed from the palette, the card is very likely off screen: the reader
  // pressed a row in a dialog and the question is at the foot of the page.
  // `nearest` because a confirmation already in view must not jump.
  const wasConfirming = useRef(confirming);
  useEffect(() => {
    if (confirming && !wasConfirming.current) {
      card.current?.scrollIntoView({ block: "nearest" });
    }
    wasConfirming.current = confirming;
  }, [confirming]);

  const isPrivate = members.data?.visibility === "private";
  // `readOnly` is a certainty this side already holds, unlike a per-domain
  // right: the control is shown shut, with the reason as its accessible
  // description, rather than removed. See `READ_ONLY_REASON` itself.
  const disabledReason = capabilities.readOnly ? READ_ONLY_REASON : undefined;
  const visibilityLabel = isPrivate ? "Share with everyone" : "Make private";

  return (
    <section
      ref={card}
      aria-labelledby={headingId}
      className="flex flex-col gap-4 rounded border border-red-300 p-4 dark:border-red-800"
    >
      <h2 id={headingId} className="text-section">
        Danger zone
      </h2>

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

      <div className="flex flex-col gap-1">
        <DestructiveAction
          label="Unregister domain"
          confirmLabel="Confirm unregister"
          pending={unregister.isPending}
          requireMatch={domain}
          confirming={confirming}
          onConfirmingChange={onConfirmingChange}
          onConfirm={() => {
            setProblem(null);
            setNotice(null);
            unregister.mutate();
          }}
        />
        {/*
          What the second step says is not one sentence but two, and which one
          it is is a fact about the domain rather than a softening: a file
          domain keeps its markdown on disk and can be registered again from
          it, while a virtual domain's engrams are the database's and go with
          it. Saying "the files stay" over a virtual domain would be the app
          telling somebody their engrams are safe on the way to deleting them.

          A `kind` of null - a listing that has not landed - falls back to the
          file sentence, because virtual is the kind that has to be declared
          and every domain this app has ever registered from a folder answers
          `file`.
        */}
        {confirming && (
          <p className="text-caption text-slate-500 dark:text-slate-400">
            {kind === "virtual"
              ? "This domain's engrams live in the database and will be deleted with it; this cannot be undone, so download the archive first if you need a copy."
              : "The files stay on disk. This instance forgets the domain and drops it from search; registering the folder again brings it back."}
          </p>
        )}
      </div>

      {/*
        Drawn once the members read has landed, because which way this control
        points is that read's answer: offering "Make private" over a domain
        that already is one would be the card guessing in place of asking.
      */}
      {members.data !== undefined && (
        <div className="flex flex-col gap-1">
          <DestructiveAction
            label={visibilityLabel}
            confirmLabel={`Confirm ${visibilityLabel.toLowerCase()}`}
            pending={visibility.isPending}
            disabledReason={disabledReason}
            requireMatch={domain}
            onConfirm={() => {
              setProblem(null);
              setNotice(null);
              visibility.mutate(!isPrivate);
            }}
          />
          {/*
            The honest disk-truth sentence, beside the control it is about: a
            private domain is protected from the other accounts on this
            instance and from nobody else, and opening one throws away the
            list of who was invited into it.
          */}
          <p className="text-caption text-slate-500 dark:text-slate-400">
            {isPrivate
              ? "Opening this domain forgets who was invited into it."
              : "Private domains protect from other users of this instance, not from whoever operates the machine."}
          </p>
        </div>
      )}
    </section>
  );
}
