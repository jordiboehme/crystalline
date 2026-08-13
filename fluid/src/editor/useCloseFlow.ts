/**
 * Closing an editor: the one question this app asks before an in-app
 * navigation, and the flag that rides the save an answer to it starts.
 *
 * Close leaves. It does not save first - a button that wrote the file as a side
 * effect of the way out gave one press two meanings, and the one it did not
 * announce was the one that could fail. What it does instead is refuse to lose
 * anything silently: a clean buffer goes straight out, because there is nothing
 * to keep and asking would be a question with one answer, and a dirty one is a
 * fork with all three outcomes named - keep the work, throw it away, or stay.
 *
 * Discard really discards. The draft store is this editor's safety net and it
 * is written a second after every pause in typing, so text abandoned here is
 * already in it; leaving that snapshot behind would mean the next visit offered
 * back exactly what was thrown away, which is the opposite of what the word
 * says. So the walkout clears it.
 *
 * The exit request is the delicate part, and it lives here rather than in
 * either screen because both need it and every one of its rules is easy to
 * break by accident:
 *
 *   - It rides WHICHEVER save consumes it, so an exit answered while a round
 *     trip is already out finishes on that trip rather than starting a second.
 *   - `consume` is conditional on the buffer still being what went on the wire.
 *     A save that lands after further typing is a receipt for text the author
 *     has already left behind, and walking them out on it would take the newer
 *     text off the screen under an answer that promised to keep the work.
 *   - Keep editing disarms. A save can be in the air when the question is asked
 *     again - the findings lag the buffer by the dry-run debounce - and without
 *     that the landing save would walk the author out on an exit they had just
 *     withdrawn.
 *   - So does a refused save: the author stays on the buffer that caused it,
 *     and the request dies with the save rather than being left armed for
 *     whatever lands next.
 *
 * It is split from the flow itself for an ordering reason. A screen's save
 * closure has to ask whether the write it just landed is the one an exit was
 * waiting on, and that closure is built where the session is - before the
 * screen has the session's own dirty state to hand this flow. So the request is
 * made first, passed to the save, and handed to the flow afterwards.
 */

import { useRef, useState } from "react";

/** The standing "leave when this save lands" flag, and its whole vocabulary. */
export interface ExitRequest {
  /** A save is about to go out and the author asked to leave on it. */
  arm: () => void;
  /** Whatever was riding is withdrawn. */
  disarm: () => void;
  /**
   * In a save's success handler: was this the save the exit was waiting on,
   * and does it still carry what is on screen? Clears the flag either way.
   */
  consume: (carriesWhatIsOnScreen: boolean) => boolean;
}

export function useExitRequest(): ExitRequest {
  // A ref rather than state because nothing renders differently for it: it is
  // read once, inside the save that answers the request, where the server's
  // response is what says the write landed.
  const armed = useRef(false);
  return {
    arm: () => {
      armed.current = true;
    },
    disarm: () => {
      armed.current = false;
    },
    consume: (carriesWhatIsOnScreen) => {
      const finished = armed.current && carriesWhatIsOnScreen;
      armed.current = false;
      return finished;
    },
  };
}

/** What closing needs to know about the buffer it is closing. */
export interface CloseFlowSession {
  /** Whether the buffer holds anything the file does not. */
  dirty: boolean;
  /** How many findings hold the save back as of this render. */
  hardErrors: number;
  /** The session's save request - the one Save and Mod-S go through too. */
  requestSave: () => void;
  /** Drop this buffer's browser-stored snapshot. */
  discardDraft: () => void;
}

export interface CloseFlow {
  /** Whether the three-way question is on screen. */
  confirming: boolean;
  /** The Close control's own handler. */
  close: () => void;
  /** Keep the work: save, and leave on the server's receipt. */
  saveAndClose: () => void;
  /** Throw the work away, snapshot included, and leave. */
  discard: () => void;
  /** Stay, and disarm anything an earlier answer left riding. */
  keepEditing: () => void;
}

export function useCloseFlow(
  exit: ExitRequest,
  session: CloseFlowSession,
  /** Go, which is whatever leaving means on this screen. */
  leave: () => void,
): CloseFlow {
  const [confirming, setConfirming] = useState(false);

  return {
    confirming,
    close: () => {
      // Every press decides afresh, so a standing exit is dropped on the way
      // in rather than inherited by whatever this press turns into.
      exit.disarm();
      if (!session.dirty) {
        leave();
        return;
      }
      setConfirming(true);
    },
    saveAndClose: () => {
      setConfirming(false);
      if (session.hardErrors > 0) {
        // The gate refuses, so this answer cannot be honoured. The question
        // goes away rather than standing there unanswerable, and the author is
        // left on their own text with the findings that hold it back in plain
        // sight - which is the one thing closing must never do quietly.
        return;
      }
      exit.arm();
      session.requestSave();
    },
    discard: () => {
      setConfirming(false);
      exit.disarm();
      session.discardDraft();
      leave();
    },
    keepEditing: () => {
      // Disarmed FIRST: staying means staying, whatever is still in the air.
      exit.disarm();
      setConfirming(false);
    },
  };
}
