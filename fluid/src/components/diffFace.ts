/**
 * What the diff pane shows for one change, reasoned about rather than drawn:
 * an editor over both sides, or one sentence where an editor would say
 * nothing (a binary file, a side the server withheld).
 *
 * Its own module so the component module exports one component and fast
 * refresh stays reliable, the same split `changes.ts` makes for the list.
 * Named for the face rather than for the pane, because a module whose name
 * differs from `DiffPane.tsx` only in case is a different file on Linux and
 * the same one on this machine: the resolver picks the `.ts` either way and
 * the component silently stops existing.
 */

import type { ChangeDetail } from "../api/admin";
import { formatBytes } from "../format";

export type PaneLayout = "unified" | "split";

export type PaneFace =
  | { kind: "editor"; layout: PaneLayout; base: string; current: string }
  | { kind: "sentence"; text: string; hint: string | null };

/** Tailwind's `lg`, the breakpoint the engram page lays its columns out at. */
export const SPLIT_QUERY = "(min-width: 64rem)";

/** The kind as a word: the change badges' own vocabulary. */
export function kindWord(kind: string): string {
  switch (kind) {
    case "added":
      return "Added";
    case "modified":
      return "Modified";
    case "deleted":
      return "Deleted";
    default:
      return kind === ""
        ? "Changed"
        : kind.charAt(0).toUpperCase() + kind.slice(1);
  }
}

/** "Changed, 20 KiB to 24 KiB", "Added, 20 KiB" or "Deleted, 20 KiB". */
export function sizeSentence(
  kind: string,
  before: number | null,
  after: number | null,
): string {
  if (before !== null && after !== null) {
    return `Changed, ${formatBytes(before)} to ${formatBytes(after)}`;
  }
  if (after !== null) {
    return `Added, ${formatBytes(after)}`;
  }
  if (before !== null) {
    return `Deleted, ${formatBytes(before)}`;
  }
  return kindWord(kind);
}

/**
 * Which face this change wears, at this width.
 *
 * A missing side reads as an empty document rather than as a refusal: an
 * addition is the team's nothing against this copy, a deletion is the team's
 * copy against nothing, and both are a diff somebody can read.
 */
export function paneFace(detail: ChangeDetail, wide: boolean): PaneFace {
  if (detail.binary) {
    return {
      kind: "sentence",
      text: sizeSentence(detail.kind, detail.sizeBefore, detail.sizeAfter),
      hint: null,
    };
  }
  if (detail.tooLarge) {
    // The sizes without the leading verb, so the sentence says once what
    // happened and once how big it is rather than twice what happened. A
    // change that could not state a size at all has no verb to strip, and
    // contributes nothing rather than a word run into the sentence.
    const sentence = sizeSentence(
      detail.kind,
      detail.sizeBefore,
      detail.sizeAfter,
    );
    const sizes = sentence.replace(/^(Changed|Added|Deleted)/, "");
    return {
      kind: "sentence",
      text: `Too large to show here${sizes === sentence ? "" : sizes}`,
      hint: `crystalline origin diff <domain> --path ${detail.path}`,
    };
  }
  return {
    kind: "editor",
    layout: wide ? "split" : "unified",
    base: detail.base ?? "",
    current: detail.current ?? "",
  };
}
