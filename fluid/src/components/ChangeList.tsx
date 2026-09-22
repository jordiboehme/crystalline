/**
 * The files a share would carry, grouped by what happened to them, each with a
 * box saying whether it goes.
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
 * The checkboxes are what make this a decision rather than a report. A share
 * used to carry the whole delta, which on a shared instance means carrying
 * everybody's afternoon; now each file says whether it goes, and the box on a
 * group heading is the way back to all of it in one press - it reads as
 * indeterminate while some of its group is ticked, so a reader can see at a
 * glance that a group is partly in.
 *
 * Every row is also the way into what actually changed in it: the path is a
 * button that opens the diff, and a menu beside it offers that same thing by
 * name plus the one destructive verb a row has, which is discarding the file
 * back to the team's copy. A discard that was refused says so under its own
 * row rather than in a line above the list, because the reason belongs to the
 * file and nothing else on the screen is about it.
 *
 * Generated folder listings are the one thing kept out of the groups and never
 * drawn: an `index.md` is rebuilt from the engrams beside it and follows the
 * domain's own configuration, so the list says nothing about it at all.
 */

import { MoreHorizontal } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import type { ReactElement } from "react";
import { useEffect, useId, useRef, useState } from "react";

import type { ShareChange } from "../api/admin";
import { substantive } from "./changes";
import { ITEM_CLASSES, MENU_CLASSES } from "./menu";
import { CHIP_VARIANTS, FOCUS_RING, IconButton } from "./primitives";

/**
 * How many paths a group draws before it starts counting the rest.
 *
 * Enough to recognize what the group is about - a folder, a tag sweep, one
 * engram and its attachments - and few enough that three groups still fit the
 * box without scrolling.
 */
const SHOWN_PER_GROUP = 5;

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
function groupChanges(
  changes: ShareChange[],
): { kind: string; paths: string[] }[] {
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

/** The box on a group heading, which is a third state as often as a second. */
function GroupCheckbox({
  label,
  checked,
  indeterminate,
  onChange,
}: {
  label: string;
  checked: boolean;
  indeterminate: boolean;
  onChange: (next: boolean) => void;
}): ReactElement {
  const box = useRef<HTMLInputElement>(null);
  // `indeterminate` is a property with no attribute, so it is set on the node
  // rather than rendered. Without it a partly-ticked group would draw as
  // unticked and one press would look like it did nothing.
  useEffect(() => {
    if (box.current !== null) {
      box.current.indeterminate = indeterminate;
    }
  }, [indeterminate]);
  return (
    <input
      ref={box}
      type="checkbox"
      aria-label={label}
      checked={checked}
      onChange={(event) => {
        onChange(event.target.checked);
      }}
      className="size-3.5 shrink-0 accent-accent-600 dark:accent-accent-400"
    />
  );
}

/**
 * One file: whether it goes, what happened to it, the way into its diff, and
 * the menu holding the verbs that do not fit on a row.
 *
 * The trigger's own element is handed to `onDiscard` rather than left to the
 * caller to read off `document.activeElement`. Radix fires `onSelect` while
 * the menu item still holds the keyboard and returns focus to the trigger
 * afterwards, so the active element at that moment is a row the browser is
 * about to unmount - and the confirmation strip needs somewhere real to put
 * the keyboard back when somebody changes their mind.
 */
function ChangeRow({
  path,
  kind,
  checked,
  refusal,
  onToggle,
  onOpen,
  onDiscard,
}: {
  path: string;
  kind: string;
  checked: boolean;
  refusal: string | null;
  onToggle: (path: string, next: boolean) => void;
  onOpen: (path: string) => void;
  onDiscard?: ((path: string, from: HTMLElement | null) => void) | undefined;
}): ReactElement {
  const trigger = useRef<HTMLButtonElement>(null);
  // The row's own refusal, named so the tick box can point at it: a reason
  // drawn beside a row is read out with the row rather than left for somebody
  // who cannot see it to wonder about.
  const refusalId = useId();
  return (
    <li className="flex flex-col gap-0.5">
      <div className="flex items-center gap-2">
        <input
          type="checkbox"
          aria-label={path}
          aria-describedby={refusal !== null ? refusalId : undefined}
          checked={checked}
          onChange={(event) => {
            onToggle(path, event.target.checked);
          }}
          className="size-3.5 shrink-0 accent-accent-600 dark:accent-accent-400"
        />
        <ChangeKindBadge kind={kind} />
        {/* The path is the way into the diff: a button named by it, in
            the same face the text wore, so a row reads as it did and
            a press on it says what changed. */}
        <button
          type="button"
          onClick={() => {
            onOpen(path);
          }}
          className={`min-w-0 truncate rounded px-0.5 text-left font-mono text-xs break-all underline-offset-2 hover:underline ${FOCUS_RING}`}
        >
          {path}
        </button>
        <DropdownMenu.Root>
          <DropdownMenu.Trigger asChild>
            <IconButton
              ref={trigger}
              label={`Actions for ${path}`}
              icon={MoreHorizontal}
              className="ml-auto h-6 w-6"
            />
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content
              align="end"
              sideOffset={6}
              className={MENU_CLASSES}
            >
              <DropdownMenu.Item
                className={ITEM_CLASSES}
                onSelect={() => {
                  onOpen(path);
                }}
              >
                What changed
              </DropdownMenu.Item>
              {onDiscard !== undefined && (
                <>
                  <DropdownMenu.Separator className="my-1 h-px bg-slate-200 dark:bg-slate-700" />
                  <DropdownMenu.Item
                    className={`${ITEM_CLASSES} text-red-700 dark:text-red-300`}
                    onSelect={() => {
                      onDiscard(path, trigger.current);
                    }}
                  >
                    Discard
                  </DropdownMenu.Item>
                </>
              )}
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      </div>
      {refusal !== null && (
        <p
          id={refusalId}
          className="pl-10 text-caption text-red-700 dark:text-red-300"
        >
          {refusal}
        </p>
      )}
    </li>
  );
}

/** One kind's heading, its first few paths, and the rest behind a press. */
function ChangeGroup({
  kind,
  paths,
  selected,
  onToggle,
  onToggleGroup,
  onOpen,
  onDiscard,
  refusals,
}: {
  kind: string;
  paths: string[];
  selected: ReadonlySet<string>;
  onToggle: (path: string, next: boolean) => void;
  onToggleGroup: (paths: string[], next: boolean) => void;
  onOpen: (path: string) => void;
  onDiscard?: ((path: string, from: HTMLElement | null) => void) | undefined;
  refusals: ReadonlyMap<string, string>;
}): ReactElement {
  const [expanded, setExpanded] = useState(false);
  const face = faceFor(kind);
  const shown = expanded ? paths : paths.slice(0, SHOWN_PER_GROUP);
  const rest = paths.length - SHOWN_PER_GROUP;
  const ticked = paths.filter((path) => selected.has(path)).length;
  return (
    <div className="flex flex-col gap-1">
      {/* The shape of the share, before any of its detail: what happened, and
          to how many - with the one control that takes a whole group in or
          out at once. */}
      <p className="flex items-center gap-2 text-caption font-medium text-slate-600 dark:text-slate-300">
        <GroupCheckbox
          label={`Share all ${face.word.toLowerCase()}`}
          checked={ticked === paths.length}
          indeterminate={ticked > 0 && ticked < paths.length}
          onChange={(next) => {
            onToggleGroup(paths, next);
          }}
        />
        {`${face.word} ${String(paths.length)}`}
      </p>
      <ul className="flex flex-col gap-0.5">
        {shown.map((path) => (
          <ChangeRow
            key={path}
            path={path}
            kind={kind}
            checked={selected.has(path)}
            refusal={refusals.get(path) ?? null}
            onToggle={onToggle}
            onOpen={onOpen}
            onDiscard={onDiscard}
          />
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
 * Every file a share would carry, by kind, inside a box that cannot grow past
 * the dialog it sits in, with nothing beneath them about the listings a share
 * carries along.
 *
 * `selected` is the caller's, not this component's: the dialog posts it, so it
 * holds it. What is drawn here is that set and the two ways to change it, one
 * file and one group at a time.
 */
export function ChangeList({
  changes,
  selected,
  onToggle,
  onToggleGroup,
  hint,
  onOpen,
  onDiscard,
  refusals,
}: {
  changes: ShareChange[];
  selected: ReadonlySet<string>;
  onToggle: (path: string, next: boolean) => void;
  onToggleGroup: (paths: string[], next: boolean) => void;
  /** Why the boxes opened the way they did, when that needs saying. */
  hint?: string | null;
  /** Open the diff pane for a path. */
  onOpen: (path: string) => void;
  /** Arm the discard strip for one path; absent when the caller may not discard. */
  onDiscard?: ((path: string, from: HTMLElement | null) => void) | undefined;
  /** The reason a path's last discard was refused, drawn under it. */
  refusals: ReadonlyMap<string, string>;
}): ReactElement | null {
  const groups = groupChanges(substantive(changes));
  if (groups.length === 0) {
    return null;
  }
  return (
    // The ceiling is the point: the groups above it say how much there is, so
    // the box scrolls rather than pushing the fields and the button off the
    // screen on a share nobody sized beforehand.
    <div className="flex max-h-56 flex-col gap-3 overflow-y-auto pr-1 text-sm">
      {groups.map((group) => (
        <ChangeGroup
          key={group.kind}
          kind={group.kind}
          paths={group.paths}
          selected={selected}
          onToggle={onToggle}
          onToggleGroup={onToggleGroup}
          onOpen={onOpen}
          onDiscard={onDiscard}
          refusals={refusals}
        />
      ))}
      {(hint ?? null) !== null && (
        <p className="text-caption text-slate-500 dark:text-slate-400">
          {hint}
        </p>
      )}
    </div>
  );
}
