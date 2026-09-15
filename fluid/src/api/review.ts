/**
 * Whether a domain reviews changes before they land, and what happens to each
 * actor's private drafts on the way back out.
 *
 * One route answers both directions (`PUT /domains/{domain}/review`), and the
 * body says which question is being asked: a `direct` body with no `folds` key
 * is the plan and writes nothing, one with a `folds` key is the change. That is
 * the server's own rule rather than this side's convention, so the two
 * functions below are two spellings of one call and cannot disagree about it.
 *
 * There is no read route. The mode a domain is in comes off the domain listing
 * every screen already makes, so a card draws its state from a fetch nobody
 * added for it, and the plan is asked for only when somebody is actually about
 * to leave review mode - which is the moment it has to be fresh anyway, since
 * it is about what other people are holding right now.
 */

import { api, encodeSegment } from "./client";

/** One draft in the plan: where it is, what it answers to, and what it is. */
export interface PlannedDraft {
  /** The domain-relative path this draft is of. */
  path: string;
  /**
   * What kind of thing stands there: `"file"` for something the actor wrote
   * beside their pages - an attachment - and absent for an engram.
   *
   * Absent rather than `"engram"`, because an engram row is exactly the shape
   * it always was: this key is the discriminator a reader added afterwards,
   * and every reader that predates it goes on reading the rows it knew.
   */
  kind?: "file";
  /**
   * The address it answers to, absent on a file: bytes answer to no address,
   * so nothing about a file can collide with one.
   */
  permalink?: string;
  /** Whether it is this actor's deletion of the engram at that path. */
  tombstone: boolean;
  /**
   * Why this draft cannot be folded, in the server's own words, or null when
   * nothing is in its way. Choice-independent: an address another engram
   * already holds is in the way however anybody else answers.
   */
  conflict: string | null;
}

/** One actor's drafts, as the plan names them. */
export interface PlannedActor {
  actor: string;
  /** How many draft changes they are holding: pages plus files. */
  entries: number;
  drafts: PlannedDraft[];
}

/** A path more than one actor is drafting: at most one of them may be folded. */
export interface ContestedPath {
  path: string;
  actors: string[];
}

/**
 * An address two actors' different paths would both claim: at most one of them
 * may be folded, because one engram answers to one address.
 *
 * The other kind of trouble a fold runs into, and it arrives separately because
 * it is a different question: neither draft is in the folder for the other
 * one's `conflict` to find, and the paths differ so `contested_paths` says
 * nothing either. A surface that drew one of the two and not the other would
 * show a clean plan for a fold that is then refused.
 */
export interface ContestedAddress {
  permalink: string;
  paths: string[];
  actors: string[];
}

/** What leaving review mode would end. */
export interface ReviewPlan {
  domain: string;
  /** The mode the domain is in now: `"overlay"` while it reviews, else null. */
  review: string | null;
  /** False on a plan, which is every answer this shape is used for. */
  applied: boolean;
  actors: PlannedActor[];
  contested_paths: ContestedPath[];
  contested_addresses: ContestedAddress[];
}

/** What one actor's drafts become. */
export type FoldChoice = "fold" | "discard";

/** Turn review mode on for a domain. Carries no choices: there are no drafts yet. */
export async function enableReview(domain: string): Promise<void> {
  await api(`/domains/${encodeSegment(domain)}/review`, {
    method: "PUT",
    body: JSON.stringify({ mode: "overlay" }),
  });
}

/** Ask what leaving review mode would end. Writes nothing. */
export async function fetchReviewPlan(domain: string): Promise<ReviewPlan> {
  return api<ReviewPlan>(`/domains/${encodeSegment(domain)}/review`, {
    method: "PUT",
    body: JSON.stringify({ mode: "direct" }),
  });
}

/**
 * Leave review mode, with one choice per actor holding drafts. Every actor the
 * plan named has to be here and nobody else; the server refuses otherwise, in
 * words that name who is missing.
 */
export async function leaveReview(
  domain: string,
  folds: Record<string, FoldChoice>,
): Promise<void> {
  await api(`/domains/${encodeSegment(domain)}/review`, {
    method: "PUT",
    body: JSON.stringify({ mode: "direct", folds }),
  });
}
