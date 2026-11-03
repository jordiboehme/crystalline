/**
 * The "Callout kind" menu: the six kinds, then how the callout folds. Picking
 * a kind writes only the kind word, picking a fold only the sign
 * (`calloutKindChanges`, `calloutFoldChanges`); picking what is checked
 * writes nothing. A details callout always folds, so it has no "Fold: none".
 */

import type { ReactElement } from "react";
import { useState } from "react";

import { CALLOUT_WORDS } from "../../components/calloutFace";
import { MENU_SEPARATOR } from "../../components/menu";
import type { CalloutKind } from "../../markdown/callouts";
import { CALLOUT_KINDS } from "../../markdown/callouts";
import {
  calloutAt,
  calloutFoldChanges,
  calloutKindChanges,
} from "../elementContext";
import { editAt } from "../elementEdit";
import type { PopoverBodyProps } from "./ElementPopover";
import { RadioItem } from "./TaskMenu";

type Fold = "open" | "closed" | null;

const FOLDS: { fold: Fold; name: string }[] = [
  { fold: null, name: "Fold: none" },
  { fold: "open", name: "Fold: open" },
  { fold: "closed", name: "Fold: closed" },
];

export function CalloutKindMenu({
  view,
  edit,
  commit,
}: PopoverBodyProps): ReactElement {
  const [current] = useState(() => {
    const callout = calloutAt(view.state, editAt(edit));
    const kind: CalloutKind = callout?.callout ?? "note";
    // A details callout with no sign folds closed by its kind alone.
    const fold: Fold = callout?.fold ?? (kind === "details" ? "closed" : null);
    return { kind, fold };
  });
  const pickKind = (kind: CalloutKind) => {
    if (kind === current.kind) {
      commit(() => []);
      return;
    }
    commit((state, now) =>
      now.kind === "callout-kind"
        ? calloutKindChanges(state, now.at, kind)
        : null,
    );
  };
  const pickFold = (fold: Fold) => {
    if (fold === current.fold) {
      commit(() => []);
      return;
    }
    commit((state, now) =>
      now.kind === "callout-kind"
        ? calloutFoldChanges(state, now.at, fold)
        : null,
    );
  };
  return (
    <>
      <div role="group" aria-label="Kind">
        {CALLOUT_KINDS.map((kind) => (
          <RadioItem
            key={kind}
            name={CALLOUT_WORDS[kind]}
            checked={kind === current.kind}
            onPick={() => {
              pickKind(kind);
            }}
          />
        ))}
      </div>
      <div role="separator" className={MENU_SEPARATOR} />
      <div role="group" aria-label="Fold">
        {FOLDS.filter(
          ({ fold }) => !(fold === null && current.kind === "details"),
        ).map(({ fold, name }) => (
          <RadioItem
            key={name}
            name={name}
            checked={fold === current.fold}
            onPick={() => {
              pickFold(fold);
            }}
          />
        ))}
      </div>
    </>
  );
}
