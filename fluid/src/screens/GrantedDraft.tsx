/**
 * Where a share-link lands: `/draft/<token>`.
 *
 * One screen for the two states a grant has, because they are two states of
 * one thing rather than two pages. Arriving presents the link, which binds it
 * to this account for good and hands back the draft; from there the page shows
 * what its author wrote, and - if this account may write on the domain at all -
 * offers the second step. Joining is that step, and it is explicit: being handed
 * somebody's unfolded work to read is not agreeing to type into it.
 *
 * Read-only is not an error state and is not drawn as one. A viewer account
 * gets the draft and the server's own sentence about why the buffer will not
 * take their typing, which names what would change it. That is the whole of
 * what a refusal owes somebody who was invited to look.
 *
 * The buffer is a plain textarea rather than the CodeMirror surface the engram
 * editor rides. A granted draft is one page handed over by its author, not a
 * corner of the knowledge base opened up: there is no vocabulary to complete
 * against, no backlinks to draw and no neighbours to advise on, and pulling
 * the editor's whole chunk in to provide none of them would tax the one screen
 * most likely to be opened cold, from a link, by somebody who has never loaded
 * this app before.
 */

import { useMutation } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useCallback, useEffect, useState } from "react";
import { useParams } from "react-router";

import { problemDetail } from "../api/client";
import type { HeldJoin } from "../api/draftLinks";
import {
  JOIN_CHANGED_EVENT,
  acceptDraftLink,
  heldJoin,
  joinDraft,
  rememberJoin,
  saveJoinedDraft,
} from "../api/draftLinks";
import type { AcceptedDraft } from "../api/model";

const BUTTON_CLASSES =
  "rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800";

export default function GrantedDraft(): ReactElement {
  const { token = "" } = useParams();
  const [draft, setDraft] = useState<AcceptedDraft | null>(null);
  const [buffer, setBuffer] = useState("");
  const [checksum, setChecksum] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // The draft this window is inside, if any - which need not be THIS one. A
  // window joined to one draft and then sent a link to another must not draw
  // the second as editable: a save would aim at the join actually held and
  // write the wrong page. So the join is compared against the draft on screen
  // rather than merely counted.
  const [held, setHeld] = useState<HeldJoin | null>(() => heldJoin());
  const joined = held !== null && draft !== null && held.path === draft.path;

  // The same event the frame's bar fires on, because the bar is where Leave
  // lives: a screen that did not listen would keep an editable buffer and a
  // live Save button after the join behind them had ended.
  const refreshHeld = useCallback(() => {
    setHeld(heldJoin());
  }, []);
  useEffect(() => {
    window.addEventListener(JOIN_CHANGED_EVENT, refreshHeld);
    return () => {
      window.removeEventListener(JOIN_CHANGED_EVENT, refreshHeld);
    };
  }, [refreshHeld]);

  /** Take what a link or a join answered with, and settle the whole screen on it. */
  const settle = (opened: AcceptedDraft): void => {
    setDraft(opened);
    setBuffer(opened.content);
    setChecksum(opened.checksum);
    setProblem(null);
  };

  const accept = useMutation({
    mutationFn: () => acceptDraftLink(token),
    onSuccess: settle,
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  // Presenting the link is a write - it binds the grant to this account - so
  // it is a mutation fired once on arrival rather than a query that could be
  // refetched behind somebody's back.
  const present = accept.mutate;
  useEffect(() => {
    if (token) present();
  }, [token, present]);

  const join = useMutation({
    mutationFn: () => joinDraft(token),
    onSuccess: (opened) => {
      settle(opened);
      if (opened.join_key) {
        // `rememberJoin` fires the change event and the listener above turns
        // it into this screen's state: one path in, rather than a second copy
        // set here that could disagree with what storage holds.
        rememberJoin({
          key: opened.join_key,
          domain: opened.domain,
          path: opened.path,
          owner: opened.owner,
          permalink: opened.permalink,
        });
      }
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const save = useMutation({
    mutationFn: async () => {
      // Re-read rather than close over the state: Leave can have happened in
      // the bar between the button being drawn and being pressed, and a save
      // aimed at a join that has ended must fail here rather than at the
      // server with a sentence about somebody else's page.
      const now = heldJoin();
      if (!now || !draft || now.path !== draft.path) {
        throw new Error("this window is not inside that draft any more");
      }
      return saveJoinedDraft(now, buffer, checksum);
    },
    onSuccess: (saved) => {
      setChecksum(saved.checksum);
      setProblem(null);
      setNotice(saved.joined ?? "Saved");
    },
    onError: (error: Error) => {
      setNotice(null);
      setProblem(problemDetail(error));
    },
  });

  if (!draft) {
    return (
      <section className="flex flex-col gap-2">
        <h1 className="text-lg font-semibold">A shared draft</h1>
        {problem ? (
          <p role="alert" className="text-sm text-red-700 dark:text-red-300">
            {problem}
          </p>
        ) : (
          <p className="text-sm text-slate-500 dark:text-slate-400">
            Opening the draft
          </p>
        )}
      </section>
    );
  }

  return (
    <section className="flex flex-col gap-3">
      <header className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h1 className="text-lg font-semibold">{draft.path}</h1>
          <p className="text-sm text-slate-500 dark:text-slate-400">
            {draft.owner}&apos;s draft in {draft.domain}, shared with you.
          </p>
        </div>
        {draft.editable && !joined && (
          <button
            type="button"
            disabled={join.isPending}
            onClick={() => {
              join.mutate();
            }}
            className={BUTTON_CLASSES}
          >
            Join this draft
          </button>
        )}
        {joined && (
          <button
            type="button"
            disabled={save.isPending}
            onClick={() => {
              save.mutate();
            }}
            className={BUTTON_CLASSES}
          >
            Save
          </button>
        )}
      </header>

      {draft.reason && (
        <p
          role="status"
          className="rounded bg-slate-100 px-2 py-1 text-sm text-slate-700 dark:bg-slate-800 dark:text-slate-200"
        >
          {draft.reason}
        </p>
      )}
      {problem && (
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problem}
        </p>
      )}
      {notice && !problem && (
        <p className="text-sm text-slate-500 dark:text-slate-400">{notice}</p>
      )}

      <textarea
        aria-label={`${draft.owner}'s draft of ${draft.path}`}
        readOnly={!joined}
        value={buffer}
        onChange={(event) => {
          setBuffer(event.target.value);
        }}
        rows={24}
        className="w-full rounded border border-slate-300 bg-transparent p-2 font-mono text-sm focus-visible:ring-2 focus-visible:ring-accent-600 focus-visible:outline-none dark:border-slate-700 dark:focus-visible:ring-accent-400"
      />
    </section>
  );
}
