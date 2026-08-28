/**
 * The files a share would carry, grouped by what happened to them.
 *
 * A flat line per file is fine for the handful somebody edited by hand and
 * useless for what a sweep produces: an evolve pass or an ingest lands dozens
 * to hundreds of paths at once, and a list of them is a wall that says nothing
 * about the shape of the share. Grouped, the first thing a reader gets is the
 * shape - three added, a hundred and twenty-one modified, one deleted - and the
 * paths sit under it as the detail behind the number.
 *
 * Each group draws its first few and counts the rest behind one press, and the
 * whole list lives in a box with a ceiling, so a share of five hundred files
 * makes the same dialog as a share of five. Nothing is hidden that cannot be
 * asked for: expanding a group shows all of it, inside that same box.
 *
 * The kind is drawn the way source control has drawn it for years: one letter
 * in a colored square, A / M / D / R, with the word itself as the accessible
 * name. The letter is what carries the meaning and the color only reinforces
 * it, because a reader who cannot tell the green from the amber still reads an
 * A and an M. The badge sits on every row rather than once on the heading: the
 * box scrolls, and a row that has been scrolled away from its heading still has
 * to say what it is.
 *
 * Generated folder listings are the one thing kept out of the groups. An
 * `index.md` is rebuilt from the engrams beside it so a team repository stays
 * browsable on the forge, and it travels with a share for that reason alone: a
 * sweep that touched forty folders would put forty derived paths in front of a
 * reader looking for the three engrams they wrote. So they are counted rather
 * than listed, in one muted line under the groups, and the line is absent
 * entirely when there are none.
 */

import type { ReactElement } from "react";
import { useState } from "react";

import { CHIP_VARIANTS } from "./primitives";

/**
 * How many paths a group draws before it starts counting the rest.
 *
 * Enough to recognize what the group is about - a folder, a tag sweep, one
 * engram and its attachments - and few enough that three groups still fit the
 * box without scrolling.
 */
const SHOWN_PER_GROUP = 5;

/** One change, as the share plan reports it. */
export interface Change {
  path: string;
  kind: string;
}

/**
 * The one face the chips have no name for: a rename is neither good news nor a
 * caution, and blue is what source control has drawn it in for years. Written
 * in the shape the chip faces are written in - the same two steps, the same
 * inversion in the dark scheme - so it sits beside them without reading as a
 * second vocabulary. blue-800 on blue-100 is 8.15:1 and blue-300 on blue-950 is
 * 10.44:1, both clear of the 4.5:1 floor for text this size.
 */
const RENAMED_FACE =
  "bg-blue-100 text-blue-800 dark:bg-blue-950 dark:text-blue-300";

/**
 * The letter, the word and the face each kind the engine writes wears.
 *
 * The faces are the chip table's own rather than a copy of its strings, so a
 * palette retune reaches the badges and the chips together: added is
 * `positive`, modified is `caution`, deleted is `danger` and anything this side
 * has not been taught is `neutral`. Only the rename has a face of its own,
 * above.
 */
const KINDS: Record<string, { letter: string; word: string; classes: string }> =
  {
    added: { letter: "A", word: "Added", classes: CHIP_VARIANTS.positive },
    modified: { letter: "M", word: "Modified", classes: CHIP_VARIANTS.caution },
    deleted: { letter: "D", word: "Deleted", classes: CHIP_VARIANTS.danger },
    renamed: { letter: "R", word: "Renamed", classes: RENAMED_FACE },
  };

/** The order the taught kinds are read in; anything else follows, as it came. */
const KIND_ORDER = ["added", "modified", "deleted", "renamed"];

/**
 * How a kind is drawn.
 *
 * A word this side has not been taught is somebody else's vocabulary rather
 * than a malformed one, so it keeps the word it arrived as, takes its own
 * initial and wears the neutral face - the same tolerance every reader in
 * `api/` shows. A report that carried no kind at all says so in a word instead
 * of drawing a gap.
 */
function faceFor(kind: string): {
  letter: string;
  word: string;
  classes: string;
} {
  const known = KINDS[kind];
  if (known) {
    return known;
  }
  const neutral = CHIP_VARIANTS.neutral;
  return kind === ""
    ? { letter: "?", word: "Changed", classes: neutral }
    : { letter: kind.slice(0, 1).toUpperCase(), word: kind, classes: neutral };
}

/**
 * One kind, as a letter in a square, with the word for anything that reads the
 * page rather than looks at it.
 */
export function ChangeKindBadge({ kind }: { kind: string }): ReactElement {
  const face = faceFor(kind);
  return (
    <span
      role="img"
      aria-label={face.word}
      className={`inline-flex h-4 w-4 shrink-0 items-center justify-center rounded font-mono text-caption leading-none font-semibold ${face.classes}`}
    >
      {face.letter}
    </span>
  );
}

/** The changes of one kind, in the order the plan listed them. */
function groupChanges(changes: Change[]): { kind: string; paths: string[] }[] {
  const groups = new Map<string, string[]>();
  for (const change of changes) {
    const paths = groups.get(change.kind);
    if (paths === undefined) {
      groups.set(change.kind, [change.path]);
    } else {
      paths.push(change.path);
    }
  }
  // Taught kinds in their own order, everything else after them in the order
  // it arrived: `sort` is stable, so an untaught kind keeps its place among
  // the other untaught ones rather than being reordered by name.
  return [...groups.entries()]
    .map(([kind, paths]) => ({ kind, paths }))
    .sort((left, right) => rank(left.kind) - rank(right.kind));
}

function rank(kind: string): number {
  const at = KIND_ORDER.indexOf(kind);
  return at < 0 ? KIND_ORDER.length : at;
}

/** One kind's heading, its first few paths, and the rest behind a press. */
function ChangeGroup({
  kind,
  paths,
}: {
  kind: string;
  paths: string[];
}): ReactElement {
  const [expanded, setExpanded] = useState(false);
  const face = faceFor(kind);
  const shown = expanded ? paths : paths.slice(0, SHOWN_PER_GROUP);
  const rest = paths.length - SHOWN_PER_GROUP;
  return (
    <div className="flex flex-col gap-1">
      {/* The shape of the share, before any of its detail: what happened, and
          to how many. */}
      <p className="text-caption font-medium text-slate-600 dark:text-slate-300">
        {`${face.word} ${String(paths.length)}`}
      </p>
      <ul className="flex flex-col gap-0.5">
        {shown.map((path) => (
          <li key={path} className="flex items-center gap-2">
            <ChangeKindBadge kind={kind} />
            <span className="font-mono text-xs break-all">{path}</span>
          </li>
        ))}
      </ul>
      {rest > 0 && (
        <button
          type="button"
          aria-expanded={expanded}
          // Which group this opens, for anything reading the buttons rather
          // than the headings above them: three over-cap groups would
          // otherwise be three controls all called "and 2 more". The visible
          // text stays short, because beside its own heading it is not
          // ambiguous at all.
          aria-label={
            expanded
              ? `Show fewer ${face.word.toLowerCase()}`
              : `Show ${String(rest)} more ${face.word.toLowerCase()}`
          }
          onClick={() => {
            setExpanded((open) => !open);
          }}
          className="self-start text-caption text-slate-500 underline underline-offset-2 hover:no-underline dark:text-slate-400"
        >
          {expanded ? "Show fewer" : `and ${String(rest)} more`}
        </button>
      )}
    </div>
  );
}

/**
 * Whether a path is a generated folder listing rather than something somebody
 * wrote, read off its filename the way the engine reads it.
 */
export function isFolderIndex(path: string): boolean {
  return path === "index.md" || path.endsWith("/index.md");
}

/**
 * Every file a share would carry, by kind, inside a box that cannot grow past
 * the dialog it sits in, with the folder listings counted beneath them.
 */
export function ChangeList({
  changes,
}: {
  changes: Change[];
}): ReactElement | null {
  const indexes = changes.filter((change) => isFolderIndex(change.path)).length;
  const groups = groupChanges(
    changes.filter((change) => !isFolderIndex(change.path)),
  );
  if (groups.length === 0 && indexes === 0) {
    return null;
  }
  return (
    // The ceiling is the point: the groups above it say how much there is, so
    // the box scrolls rather than pushing the fields and the button off the
    // screen on a share nobody sized beforehand.
    <div className="flex max-h-56 flex-col gap-3 overflow-y-auto pr-1 text-sm">
      {groups.map((group) => (
        <ChangeGroup key={group.kind} kind={group.kind} paths={group.paths} />
      ))}
      {indexes > 0 && (
        // Under the groups and quieter than them, because that is exactly the
        // weight it carries: something the share does, not something the
        // reader has to decide about.
        <p className="text-caption text-slate-500 dark:text-slate-400">
          {`Also refreshes ${String(indexes)} folder ${indexes === 1 ? "index" : "indexes"}`}
        </p>
      )}
    </div>
  );
}
