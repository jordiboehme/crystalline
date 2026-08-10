/**
 * One editing session as React state: the shared document, who else is in the
 * room, what the server says about saving, and whether there is a room at all.
 *
 * The doc, the awareness and the provider are one generation, built together
 * in a single effect and torn down together. Nothing here compares epochs -
 * that is the provider's job, and by the time it reports a gap it has already
 * gone permanently silent (see `ProviderHandlers.onEpochGap`). The hook's
 * whole answer to a gap is to snapshot the text it still holds into the draft
 * store and bump a generation counter, which rebuilds the generation around
 * the server's fresh session.
 *
 * Saving is the server's: the hook never PUTs. `flush` asks the session to
 * write now, and the control channel's verdicts - saved, save-failed,
 * conflict, closed - are what the surface renders.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { ApiProblem } from "../api/client";
import { fetchEngramDetail } from "../api/engram";
import { writeDraft } from "../editor/drafts";
import { presenceColor } from "./colors";
import type {
  CollabHello,
  CollabStatus,
  ProviderHandlers,
  SocketFactory,
} from "./provider";
import { CollabProvider, TEXT_NAME, collabUrl } from "./provider";

/** How long the room's "an outside change was merged in" notice stays up. */
export const MERGE_NOTICE_MS = 4000;

/**
 * What a tab that joined DURING a conflict is told about it.
 *
 * The greeting carries `save_state: "conflict"` but no conflict body: the
 * Conflict broadcast that carried the other side's bytes went out before this
 * socket was subscribed, and the session has no way to be asked for it again.
 * So the body is re-derived from the engram's own read - the file as it
 * stands is exactly what the room is being asked to choose against - and the
 * wording says which way that read went. A 404 is the deletion; anything else
 * unreadable is admitted as unknown rather than guessed at, because "the file
 * is empty" and "I could not look" are different facts and one of them would
 * be a lie told beside a button that overwrites.
 */
const JOINED_EDIT_DETAIL =
  "This engram changed outside the session before you joined it.";
const JOINED_DELETED_DETAIL =
  "This engram's file was deleted outside the session before you joined it.";
const JOINED_UNREADABLE_DETAIL =
  "This engram changed outside the session before you joined it, and its " +
  "current text could not be read from here.";

/** What a participant's awareness state carries about them. */
interface PresenceUser {
  name?: string;
  color?: string;
  colorLight?: string;
}

export type CollabMode = "connecting" | "collab" | "solo";

export interface CollabConflict {
  kind: "edit" | "deleted";
  theirs: string | null;
  detail: string;
}

export interface CollabParticipant {
  name: string;
  color: string;
  self: boolean;
}

export interface CollabSession {
  mode: CollabMode;
  /** Set while mode is "collab". */
  ytext: Y.Text | null;
  awareness: Awareness | null;
  epoch: string | null;
  separator: "\r\n" | "\n";
  status: CollabStatus;
  saveState: "ok" | "pending" | "failed" | "conflict";
  saveDetail: string | null;
  conflict: CollabConflict | null;
  participants: CollabParticipant[];
  /** The engram's current permalink per the last save receipt. */
  permalink: string;
  flush: () => void;
  resolve: (choice: "mine" | "theirs") => void;
  /** True after the room accepted an external deletion: leave the editor. */
  closed: boolean;
  /** True briefly after a merged control: an outside change was folded into
   *  the session (the spec's room toast). Auto-clears after a few seconds. */
  mergeNotice: boolean;
}

export interface CollabSessionOptions {
  domain: string;
  permalink: string;
  /** The login name: the draft key the epoch-gap snapshot writes under. */
  account: string;
  displayName: string;
  enabled: boolean;
  /** Test seam, threaded to the provider; production callers omit it. */
  socketFactory?: SocketFactory;
}

/**
 * LF session text back into the file's own line endings, and the only place
 * on the client that spelling lives. The shared document is always LF space;
 * a file that uses CRLF gets it back here, on the way to a draft or to disk.
 */
export function fileSpace(text: string, separator: "\r\n" | "\n"): string {
  return separator === "\n" ? text : text.replace(/\n/g, separator);
}

export function useCollabSession(options: CollabSessionOptions): CollabSession {
  const {
    domain,
    permalink: address,
    account,
    displayName,
    enabled,
    socketFactory,
  } = options;
  // One doc/awareness/provider generation per (address, epoch-reset). The
  // counter is what forces a rebuild after an epoch gap.
  const [generation, setGeneration] = useState(0);
  const [mode, setMode] = useState<CollabMode>(enabled ? "connecting" : "solo");
  const [status, setStatus] = useState<CollabStatus>("connecting");
  const [hello, setHello] = useState<CollabHello | null>(null);
  const [saveState, setSaveState] = useState<CollabSession["saveState"]>("ok");
  const [saveDetail, setSaveDetail] = useState<string | null>(null);
  const [conflict, setConflict] = useState<CollabConflict | null>(null);
  const [closed, setClosed] = useState(false);
  const [mergeNotice, setMergeNotice] = useState(false);
  const [permalink, setPermalink] = useState(address);
  const [participants, setParticipants] = useState<CollabParticipant[]>([]);
  const [bound, setBound] = useState<{
    ytext: Y.Text;
    awareness: Awareness;
  } | null>(null);
  const docRef = useRef<{
    doc: Y.Doc;
    awareness: Awareness;
    provider: CollabProvider;
  } | null>(null);
  // Read by handlers that live as long as one generation, so they must not
  // close over a particular render's values. Mirrored from an effect declared
  // ABOVE the one that builds the generation, so a rebuild always sees the
  // current identity while a render never writes a ref.
  const identity = useRef({ account, displayName });
  const separatorRef = useRef<"\r\n" | "\n">("\n");
  const checksumRef = useRef("");
  /**
   * The conflict and the save verdict as they stand RIGHT NOW, written at
   * every site that changes them rather than mirrored from an effect.
   *
   * Two readers need that exactness. The control handlers live as long as one
   * generation and would otherwise close over a stale render's values, and the
   * joiner's derivation settles asynchronously: it has to see a conflict that
   * was broadcast a moment ago even if React has not flushed effects yet, and
   * it has to see a room that healed while its request was in flight.
   */
  const conflictRef = useRef<CollabConflict | null>(null);
  const saveStateRef = useRef<CollabSession["saveState"]>("ok");
  const mergeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    identity.current = { account, displayName };
  }, [account, displayName]);

  useEffect(() => {
    if (!enabled) {
      // Nothing to build and nothing to set: `mode` is derived below, so a
      // session that is switched off reads solo without a state write.
      return;
    }
    const doc = new Y.Doc();
    const ytext = doc.getText(TEXT_NAME);
    const awareness = new Awareness(doc);
    awareness.setLocalStateField("user", {
      name: identity.current.displayName,
      ...presenceColor(identity.current.account),
    });
    const readParticipants = () => {
      const room: CollabParticipant[] = [];
      for (const [clientId, state] of awareness.getStates()) {
        const user = (state as { user?: PresenceUser } | undefined)?.user;
        if (!user?.name) {
          continue;
        }
        room.push({
          name: user.name,
          // A participant with no color of their own still gets a chip; the
          // room's own palette is what the color usually comes from.
          color: user.color ?? presenceColor(user.name).color,
          self: clientId === doc.clientID,
        });
      }
      setParticipants(room);
    };
    // The save verdict, kept in a ref beside the state: the conflict
    // derivation below settles asynchronously and has to judge the room as it
    // stands when its answer arrives, not as it stood when it asked.
    const setSave = (next: CollabSession["saveState"]) => {
      saveStateRef.current = next;
      setSaveState(next);
    };
    /**
     * The conflict is over: the room converged and saving is live again.
     *
     * There is no "conflict over" control to key on, because the server does
     * not send one - a resolution is announced by what it produced. "Mine"
     * lands as the Saved receipt of the save it re-armed; "theirs" converges
     * the document and says Merged, with no save of its own. Both are handled,
     * and the save state only heals when it was the conflict that suspended
     * it: a Merged arriving in a room that is merely mid-save must not
     * overwrite "Saving..." with "Saved".
     */
    const clearConflict = () => {
      conflictRef.current = null;
      setConflict(null);
      if (saveStateRef.current === "conflict") {
        setSave("ok");
        setSaveDetail(null);
      }
    };
    const handlers: ProviderHandlers = {
      onControl: (control) => {
        switch (control.kind) {
          case "hello": {
            setHello(control);
            separatorRef.current = control.separator;
            checksumRef.current = control.checksum;
            setPermalink(control.permalink);
            // The greeting is this tab's whole picture of a room it did not
            // watch start: every verdict the server owns is adopted from it,
            // because the broadcast that carried each one went out before
            // this socket was subscribed and none of them is repeated.
            switch (control.save_state) {
              case "conflict": {
                // Joined into a suspended room: saving is off from this tab's
                // first frame, whether or not it ever sees a Conflict of its
                // own. The body is re-derived below.
                setSave("conflict");
                break;
              }
              case "failed": {
                // A room that cannot save, greeting a tab that would
                // otherwise read "Saved" over an engram nothing has written
                // since the refusal. The reason rides the greeting; a server
                // that does not send one leaves the alert wordless rather
                // than inventing a cause.
                setSave("failed");
                setSaveDetail(control.detail ?? null);
                break;
              }
              case "ok": {
                // A healthy room: whatever this tab was told before it went
                // away is over, conflict included - the resolution may have
                // been somebody else's while the socket was down.
                clearConflict();
                // Only a verdict the SERVER owns heals beyond that. "pending"
                // is this tab's own unsaved text, which no greeting knows
                // about: a resync after a drop greets with "ok" while the
                // edits made offline are still owed, and adopting it would
                // call them saved.
                if (saveStateRef.current === "failed") {
                  setSave("ok");
                  setSaveDetail(null);
                }
                break;
              }
            }
            break;
          }
          case "saved": {
            checksumRef.current = control.checksum;
            setPermalink(control.permalink);
            setSave("ok");
            setSaveDetail(null);
            // A save landing IS the end of a conflict resolved as "mine":
            // the server re-armed the flush and this is its receipt.
            clearConflict();
            break;
          }
          case "save-failed": {
            setSave("failed");
            setSaveDetail(control.detail);
            break;
          }
          case "merged": {
            setMergeNotice(true);
            if (mergeTimer.current !== null) {
              clearTimeout(mergeTimer.current);
            }
            mergeTimer.current = setTimeout(() => {
              mergeTimer.current = null;
              setMergeNotice(false);
            }, MERGE_NOTICE_MS);
            // And a converged room IS the end of a conflict resolved as
            // "theirs", which sends no Saved of its own.
            clearConflict();
            break;
          }
          case "conflict": {
            setSave("conflict");
            setSaveDetail(control.detail);
            const raised: CollabConflict = {
              kind: control.conflict_kind,
              theirs: control.theirs,
              detail: control.detail,
            };
            // The ref is written HERE rather than from an effect mirroring
            // the state: the derivation below reads it to decide whether to
            // write, and a broadcast that lands before effects flush would
            // otherwise be overwritten by a fetch that started earlier.
            conflictRef.current = raised;
            setConflict(raised);
            break;
          }
          case "closed": {
            setClosed(true);
            break;
          }
        }
      },
      onStatus: (next) => {
        setStatus(next);
        if (next === "failed") {
          // A connect that never landed a session means there is no room to
          // join and the editor opens solo - including the rebuild after an
          // epoch gap, which is a first connect of its own and which reset
          // the mode out of "collab" for exactly this reason. A session that
          // HAS been joined keeps its buffer and its binding instead; the
          // status line says it is offline and the room resyncs. Forking a
          // live room into a solo buffer would be a second history of the
          // same engram.
          setMode((current) => (current === "collab" ? "collab" : "solo"));
        }
      },
      onSynced: () => {
        // The binding is published HERE rather than when the generation is
        // built: what a caller binds an editor to is a document that has
        // agreed with the server, and before the first sync this doc is an
        // empty text that would render as an empty engram.
        setBound({ ytext, awareness });
        setMode("collab");
      },
      onEpochGap: () => {
        // The provider is already permanently silent; nothing of the old doc
        // can leak. Snapshot what the author had - the hook is the one writer
        // with the doc still in hand - then rebuild around the new epoch.
        const current = docRef.current;
        if (current !== null) {
          writeDraft(identity.current.account, domain, address, {
            content: fileSpace(
              current.doc.getText(TEXT_NAME).toJSON(),
              separatorRef.current,
            ),
            baseChecksum: checksumRef.current,
            savedAt: new Date().toISOString(),
          });
        }
        // Back to connecting, and deliberately not left on "collab": the
        // room the mode was claiming is gone, its document is about to be
        // torn down, and a mode that outlived its binding renders as a
        // skeleton with no bound left to escape into. Status likewise - the
        // provider that last said "connected" has been discarded, so reading
        // anything off it would be reading a corpse.
        setMode("connecting");
        setStatus("connecting");
        // And the verdict goes with the session that reached it. What the
        // rebuild lands on is a DIFFERENT server session that greets with a
        // state of its own, so anything this one left standing - a conflict
        // banner whose buttons would resolve a room that is gone, a refusal
        // naming a save nobody is retrying, a merge toast - is cleared here
        // rather than left up until some later control happens to overwrite
        // it. Refs as well as state: the handlers of the rebuilt generation
        // read the refs, and so does the joiner's derivation.
        //
        // Here rather than at the top of the build effect below, because this
        // is the one path that rebuilds a generation in place, and a state
        // write belongs in the event that causes it rather than in an effect.
        saveStateRef.current = "ok";
        conflictRef.current = null;
        setSaveState("ok");
        setSaveDetail(null);
        setConflict(null);
        setMergeNotice(false);
        setGeneration((current) => current + 1);
      },
    };
    const provider = new CollabProvider(
      collabUrl(domain, address),
      doc,
      awareness,
      handlers,
      socketFactory,
    );
    const onUpdate = (_update: Uint8Array, origin: unknown) => {
      if (origin === provider) {
        return; // the server's own text, not an edit waiting to be saved
      }
      setSave("pending");
      setSaveDetail(null);
    };
    doc.on("update", onUpdate);
    awareness.on("change", readParticipants);
    readParticipants();
    docRef.current = { doc, awareness, provider };
    return () => {
      awareness.off("change", readParticipants);
      doc.off("update", onUpdate);
      provider.destroy();
      doc.destroy();
      if (mergeTimer.current !== null) {
        clearTimeout(mergeTimer.current);
        mergeTimer.current = null;
      }
      docRef.current = null;
      setBound(null);
      setParticipants([]);
    };
  }, [generation, domain, address, enabled, socketFactory]);

  // The mid-conflict joiner (see the JOINED_* details above): a greeting that
  // says "conflict" with nothing on screen to resolve is a dead banner, so
  // the body is fetched once per greeting.
  useEffect(() => {
    if (hello === null || hello.save_state !== "conflict") {
      return;
    }
    if (conflictRef.current !== null) {
      return; // the broadcast reached this tab after all; it is the truth
    }
    const at = hello.permalink;
    let cancelled = false;
    void (async () => {
      let derived: CollabConflict;
      try {
        const theirs = await fetchEngramDetail(domain, at);
        derived = {
          kind: "edit",
          theirs: theirs.content,
          detail: JOINED_EDIT_DETAIL,
        };
      } catch (error) {
        derived =
          error instanceof ApiProblem && error.status === 404
            ? { kind: "deleted", theirs: null, detail: JOINED_DELETED_DETAIL }
            : { kind: "edit", theirs: null, detail: JOINED_UNREADABLE_DETAIL };
      }
      // Judged as the room stands NOW, not as it stood when the read was
      // asked for: somebody else may have resolved the whole thing while this
      // request was in flight, and a late answer must not plant a phantom
      // conflict on a healthy room. Three conditions, all live: this
      // generation is still the current one (the cleanup below), no conflict
      // has been raised meanwhile, and saving is still suspended.
      if (
        cancelled ||
        conflictRef.current !== null ||
        saveStateRef.current !== "conflict"
      ) {
        return;
      }
      conflictRef.current = derived;
      setConflict(derived);
    })();
    return () => {
      cancelled = true;
    };
    // `generation` is a dependency for its cleanup alone: a rebuilt session is
    // a different room, and whatever the old one was still fetching is void.
  }, [hello, domain, generation]);

  const flush = useCallback(() => {
    docRef.current?.provider.flush();
  }, []);
  const resolve = useCallback((choice: "mine" | "theirs") => {
    docRef.current?.provider.resolve(choice);
  }, []);

  // A session that is switched off is solo whatever the last generation left
  // behind, which is why this is derived rather than stored.
  const shown: CollabMode = enabled ? mode : "solo";
  const joined = shown === "collab";
  return {
    mode: shown,
    // Only while there is a room: a caller binding an editor to this text is
    // binding it to a session, and there is no session before one syncs.
    ytext: joined ? (bound?.ytext ?? null) : null,
    awareness: joined ? (bound?.awareness ?? null) : null,
    epoch: hello?.epoch ?? null,
    separator: hello?.separator ?? "\n",
    status,
    saveState,
    saveDetail,
    conflict,
    participants,
    permalink,
    flush,
    resolve,
    closed,
    mergeNotice,
  };
}
