/**
 * The editors' notice for a text the server kept for this page: when the
 * last person left a room that was still in a conflict, or whose save
 * failed, the server kept the unsaved text. The notice offers it the way the
 * draft banners offer a browser's own copy: see it, take it, or let it go.
 *
 * Quiet on purpose. It is there when the page opens and waits for a choice,
 * so it is a note, not an alert, and a person with no right to the kept
 * texts (or none kept) never sees it: every refusal of the listing reads as
 * "nothing kept here".
 *
 * The server copy is the last copy until the text is saved again, so
 * Replace with it never deletes it on the spot. It reads the text, checks
 * the editor can still take it, puts it in as one change the person can
 * undo, and asks the editor whether it really landed. Then:
 *
 * - In the solo editor, which numbers its own saves, the kept copy goes with
 *   the first save that began after that, whatever the text is by then (the
 *   person chose to work on top of it). Until then the note waits for it.
 * - In a room, the server saves on its own schedule and this tab cannot tell
 *   which save holds the text, so the note never claims one did. It says the
 *   text is back and leaves Drop to the person.
 */

import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useEffect, useId, useRef, useState } from "react";

import { problemDetail } from "../api/client";
import {
  dropKeptText,
  fetchKeptText,
  isNoKeptRight,
  keptForPage,
  keptKey,
  keptNoticeSentence,
  keptQuery,
  keptTextKey,
} from "../api/kept";
import type { KeptTextEntry } from "../api/kept";
import { useAuth } from "../auth/AuthContext";
import { KeptTextDialog } from "../components/KeptTextDialog";
import { FOCUS_RING } from "../components/primitives";
import { refreshAfterWrite } from "../query/refreshAfterWrite";

const NOTICE =
  "flex flex-wrap items-center gap-x-3 gap-y-2 rounded border border-hair bg-sunken px-3 py-2 text-sm text-ink-2";
const DANGER = `rounded font-medium text-red-700 underline underline-offset-[3px] hover:no-underline dark:text-red-300 ${FOCUS_RING}`;
const ACTION = `rounded text-link underline decoration-(--link-line) underline-offset-[3px] hover:no-underline aria-disabled:cursor-default aria-disabled:opacity-50 ${FOCUS_RING}`;

/** How the solo editor tells the save after a replace apart. */
export interface SoloSaves {
  /** How many saves have started so far. */
  started: () => number;
  /** Whether a save that began after `started()` returned `count` has
   *  landed. */
  landedAfter: (count: number) => boolean;
}

/** A text put in the editor. */
interface Placed {
  id: string;
  /** The solo editor's save count when the text went in; null in a room. */
  savesBefore: number | null;
  /** A save held it, but the kept copy could not be dropped. */
  held?: boolean;
}

export function KeptTextNotice({
  domain,
  kind,
  path,
  draft,
  ready,
  onReplace,
  soloSaves,
  onDropped,
}: {
  domain: string;
  /** The kind of room this editor is. */
  kind: "engram" | "reflection";
  /** The page's path in its domain (inside `reflections/` for a page); null
   *  when this editor has no page of its own to offer a text for. */
  path: string | null;
  /** Whether the editor is over the caller's own review draft; null while
   *  that is not known yet, which offers nothing. */
  draft: boolean | null;
  /** Whether the editor can take a text now: a connected room, or a solo
   *  buffer that is mounted. Read again after the text arrives. */
  ready: boolean;
  /** Put `text` in the editor as one undoable change; true only when it
   *  really landed. */
  onReplace: (text: string) => boolean;
  /** The solo editor's saves, so the kept copy goes with the first save
   *  after the replace. Left out in a room: there the person drops it. */
  soloSaves?: SoloSaves | undefined;
  /** Told after a person's Drop went through, to hand the focus back. */
  onDropped?: () => void;
}): ReactElement | null {
  const { user, capabilities } = useAuth();
  const queryClient = useQueryClient();
  const listing = useQuery(keptQuery(domain, user !== null && path !== null));
  const [showing, setShowing] = useState<KeptTextEntry | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [placed, setPlaced] = useState<Placed | null>(null);
  const [asking, setAsking] = useState(false);
  const questionId = useId();
  const dropButton = useRef<HTMLButtonElement>(null);
  // Set when the focus belongs on Drop once it is drawn again: after Keep,
  // Escape or a drop that failed.
  const focusDrop = useRef(false);
  // The editor's readiness as it is now, for the check after the read.
  const live = useRef(false);
  const can = ready;
  useEffect(() => {
    live.current = can;
  });

  useEffect(() => {
    if (!asking && focusDrop.current) {
      focusDrop.current = false;
      dropButton.current?.focus();
    }
  });

  const refresh = () => {
    void refreshAfterWrite(queryClient, { queryKey: keptKey(domain) });
  };

  /** Delete the kept copy. `used`: its text was saved from the editor. */
  const drop = (id: string, used: boolean): Promise<boolean> =>
    dropKeptText(domain, id).then(
      () => {
        setProblem(null);
        setPlaced(null);
        queryClient.removeQueries({ queryKey: keptTextKey(domain, id) });
        refresh();
        return true;
      },
      (error: unknown) => {
        refresh();
        if (error instanceof Error && isNoKeptRight(error)) {
          // Somebody else dropped it meanwhile: it is gone either way.
          setPlaced(null);
          return true;
        }
        const why = error instanceof Error ? problemDetail(error) : "";
        if (used) {
          setPlaced((was) => (was === null ? null : { ...was, held: true }));
        }
        setProblem(
          used
            ? `Your text is saved, but the kept copy could not be dropped. ${why}`
            : why,
        );
        return false;
      },
    );

  // In the solo editor, the first save that began after the text went in
  // lets the kept copy go.
  const placedId = placed?.id ?? null;
  const savedAfter =
    placed !== null &&
    placed.held !== true &&
    placed.savesBefore !== null &&
    soloSaves !== undefined &&
    soloSaves.landedAfter(placed.savesBefore);
  useEffect(() => {
    if (placedId === null || !savedAfter) {
      return;
    }
    void drop(placedId, true);
    // `drop` is rebuilt every render; the save is what matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [savedAfter, placedId]);

  const entry =
    path === null || draft === null
      ? null
      : keptForPage(listing.data, kind, path, draft);
  if (entry === null) {
    return null;
  }
  // A read-only instance refuses the delete: the note then only shows.
  const writable = !capabilities.readOnly;
  const waiting = placed !== null && placed.id === entry.id;

  const replace = async () => {
    if (busy || !live.current) {
      return;
    }
    setBusy(true);
    try {
      let text: string;
      try {
        const read = await queryClient.fetchQuery({
          queryKey: keptTextKey(domain, entry.id),
          queryFn: () => fetchKeptText(domain, entry.id),
          retry: false,
        });
        text = read.text;
      } catch (error) {
        if (error instanceof Error && isNoKeptRight(error)) {
          // Already gone: the listing catches up and the note goes.
          setProblem(null);
          refresh();
        } else {
          setProblem(error instanceof Error ? problemDetail(error) : null);
        }
        return;
      }
      // The room may have ended or gone away while the text was on its way.
      if (!live.current) {
        setProblem("The editor cannot take the text now. Nothing changed.");
        return;
      }
      if (!onReplace(text)) {
        setProblem("The text did not reach the editor. Nothing changed.");
        return;
      }
      setProblem(null);
      setPlaced({
        id: entry.id,
        savesBefore: soloSaves === undefined ? null : soloSaves.started(),
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <aside role="note" aria-label="Kept text" className={NOTICE}>
      <span>{keptNoticeSentence(entry)}</span>
      {waiting && placed.held !== true && (
        <span role="status">
          {placed.savesBefore === null
            ? "Your text is back in the editor. Drop the kept copy once it is saved."
            : "Waiting for the save."}
        </span>
      )}
      <button
        type="button"
        className={ACTION}
        onClick={() => {
          setShowing(entry);
        }}
      >
        Show
      </button>
      {can && writable && !waiting && !asking && (
        <button
          type="button"
          className={ACTION}
          aria-disabled={busy}
          onClick={() => {
            void replace();
          }}
        >
          Replace with it
        </button>
      )}
      {writable &&
        (asking ? (
          <span
            role="group"
            aria-labelledby={questionId}
            className="inline-flex flex-wrap items-center gap-3"
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.stopPropagation();
                focusDrop.current = true;
                setAsking(false);
              }
            }}
          >
            <span id={questionId}>Drop it for good?</span>
            <button
              type="button"
              autoFocus
              className={DANGER}
              onClick={() => {
                setAsking(false);
                void drop(entry.id, false).then((gone) => {
                  if (gone) {
                    onDropped?.();
                  } else {
                    focusDrop.current = true;
                    // Draw again so the focus effect runs.
                    setAsking(false);
                    dropButton.current?.focus();
                  }
                });
              }}
            >
              Drop it
            </button>
            <button
              type="button"
              className={ACTION}
              onClick={() => {
                focusDrop.current = true;
                setAsking(false);
              }}
            >
              Keep
            </button>
          </span>
        ) : (
          <button
            ref={dropButton}
            type="button"
            aria-label="Drop the kept text"
            aria-disabled={busy}
            className={ACTION}
            onClick={() => {
              if (!busy) {
                setAsking(true);
              }
            }}
          >
            Drop
          </button>
        ))}
      {problem !== null && problem !== "" && (
        <p role="alert" className="basis-full text-red-800 dark:text-red-200">
          {problem}
        </p>
      )}
      {showing !== null && (
        <KeptTextDialog
          domain={domain}
          entry={showing}
          onClose={() => {
            setShowing(null);
          }}
        />
      )}
    </aside>
  );
}
