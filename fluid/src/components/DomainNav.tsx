/**
 * The sidebar once a domain is open: which domain you are in, and what is in
 * it.
 *
 * A domain is a place somebody works in rather than an entry they picked once,
 * so while they are inside one the sidebar stops being a list of everything and
 * becomes the way around this one thing. Two controls make that true: a
 * switcher that names the current domain and moves across to another, and the
 * folder tree below it, which is the domain's own shape rather than a flat
 * listing of it. The way back out to every domain stays on screen beside them,
 * because a place you cannot leave is a trap.
 *
 * The tree is walked rather than downloaded. `GET /domains/{d}/tree` answers
 * one folder at a time - its subfolder names and the engrams directly in it -
 * and this asks for a folder only when somebody opens it. A deep fetch would
 * mean pulling every engram of a domain into the frame around every screen,
 * and the folder structure below the first level would still have to be
 * inferred from engram paths. Each folder is cached under the same key the
 * domain screen uses (`treeKey`), so the root costs nothing when a reader is
 * already looking at it and an expanded folder stays expanded for free.
 *
 * The folders on the way to the engram being read start open, because a mark
 * on a row nobody can see marks nothing. The folder being browsed on the
 * screen beside the tree counts as being there too: it opens, and its row
 * carries the same you-are-here mark an engram row does - until a frontmatter
 * filter goes on, when the screen leaves the folder for the whole domain and
 * no row may claim to be the page on screen. Opening and marking are two
 * questions here, and the second one is the screen's own to answer.
 *
 * A folder row is two controls rather than one. Its name is a link to that
 * folder on the domain screen, which is where a folder of any size can be
 * paged; its chevron is a button that looks inside the sidebar without moving
 * the screen. They are siblings, never nested: a link inside a button is an
 * accessibility violation, and a reader would have no way to say which of the
 * two they meant. A level the server had to cut ends in one more row - the
 * whole folder, on the screen - because the sidebar is a way around a domain
 * rather than a place to render ten thousand rows.
 */

import { useQuery } from "@tanstack/react-query";
import { Folder as FolderIcon } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";

import { problemDetail } from "../api/client";
import { treeQuery } from "../api/domain";
import type { DomainSummary } from "../api/domains";
import type { EngramRow } from "../api/engrams";
import { hasFilters } from "../api/engrams";
import { useAuth } from "../auth/AuthContext";
import { frontmatterFilters } from "../filters";
import { RETIRED_CLASS, isRetired } from "../lifecycle";
import { domainRoute, engramRoute, folderRoute, manifestRoute } from "../paths";
import { CreateEngramDialog } from "./CreateEngramDialog";
import { ITEM_CLASSES, MENU_CLASSES } from "./menu";
import { FOCUS_RING } from "./primitives";

export interface DomainNavProps {
  /** The domain the route is inside. */
  domain: string;
  /**
   * The permalink being read or edited, or the empty string on the domain's
   * own home screen, where there is none.
   */
  permalink: string;
  /** Whether the MANIFEST page or its editor is the screen open right now. */
  onManifest: boolean;
  /** Every registered domain, for the switcher. */
  domains: DomainSummary[];
}

export function DomainNav({
  domain,
  permalink,
  onManifest,
  domains,
}: DomainNavProps) {
  const { capabilities } = useAuth();
  const [creating, setCreating] = useState(false);
  const [params] = useSearchParams();
  // Which folder the screen beside the tree was last pointed at. Only the
  // domain's own screen writes `path`; `permalink` being empty is what says
  // the screen beside the tree is that one rather than an engram page, so the
  // pair is read as one state rather than two that could contradict each
  // other. (The MANIFEST page also has no permalink, and no link in this app
  // puts `?path=` on that route, so nothing is browsing there either.)
  const browsing =
    permalink === "" && !onManifest ? (params.get("path") ?? "") : "";
  // Which folder may call itself the page the reader is on - not the same
  // question. Under a frontmatter filter the screen leaves the folder and
  // lists the whole domain, so no folder is the current page: the mark would
  // name a page nobody is on, on a link that drops the filter when it is
  // followed. The predicate is the screen's own (`hasFilters` over
  // `frontmatterFilters`), read from the same URL, so the frame and the screen
  // cannot disagree about which view is up. The branch still opens, because a
  // filter is a lens over the domain rather than a reason to fold away where
  // the reader just was.
  const marked = hasFilters(frontmatterFilters(params)) ? "" : browsing;

  return (
    <div className="flex flex-col gap-3">
      <Link
        to="/"
        className="px-2 text-xs text-slate-500 underline underline-offset-2 hover:no-underline focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:text-slate-400"
      >
        All domains
      </Link>

      <DomainSwitcher domain={domain} domains={domains} />

      {/*
        Pinned ahead of the tree, because a MANIFEST is what introduces the
        domain rather than something filed inside it, and drawn as an ordinary
        row: position is the thing that says it is different, and a second
        treatment on top of it would only be decoration. Its name stays in
        capitals because that is the file's actual name, not a heading style.
        The you-are-here cue is `EngramLink`'s own mechanism, `aria-current`
        plus a highlight class, so a reader on either page gets the same signal
        regardless of which row it marks. The tree drops its duplicate of this
        row below, so the domain's introduction is offered once.
      */}
      <Link
        to={manifestRoute(domain)}
        aria-current={onManifest ? "page" : undefined}
        // The tree rows' own padding, to the half unit: "drawn as an ordinary
        // row" is the rule above, and a row a shade taller than the ones under
        // it is precisely the second treatment that rule turns down.
        className={`block truncate rounded px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:hover:bg-slate-800 ${
          onManifest ? "bg-slate-100 font-medium dark:bg-slate-800" : ""
        }`}
      >
        MANIFEST
      </Link>

      <div>
        <h2 className="text-caption px-2 pb-2 font-semibold text-slate-500 dark:text-slate-400">
          Engrams
        </h2>
        <TreeBranch
          domain={domain}
          path=""
          permalink={permalink}
          browsing={browsing}
          marked={marked}
        />
        {/*
          Left out on the domain's own home screen only: that screen carries
          the same launcher beside its heading, prefilled with the folder
          being browsed, so the sidebar would otherwise offer a second one
          right next to the first. `permalink` is empty there and there
          alone - both the engram page and the editor resolve one (see
          `Layout.tsx`'s `DomainSidebar`) - so this is the only launcher on
          either of those screens.
        */}
        {capabilities.canWrite && permalink !== "" && (
          <button
            type="button"
            onClick={() => {
              setCreating(true);
            }}
            className="mt-2 w-full rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
          >
            New engram
          </button>
        )}
      </div>
      {creating && (
        <CreateEngramDialog
          domain={domain}
          initialFolder=""
          onClose={() => {
            setCreating(false);
          }}
        />
      )}
    </div>
  );
}

/**
 * Which domain this is, and the way across to another.
 *
 * Each entry carries what that domain holds, which is what makes choosing
 * between them a choice rather than a guess. A domain the listing does not
 * name - a wrong address, or one this identity may not list - still shows on
 * the trigger, so the switcher says where the reader actually is.
 */
function DomainSwitcher({
  domain,
  domains,
}: {
  domain: string;
  domains: DomainSummary[];
}) {
  const navigate = useNavigate();

  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger
        aria-label={`Domain: ${domain}`}
        className="flex w-full items-center gap-2 rounded border border-slate-300 px-2 py-1.5 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
      >
        <span className="truncate font-medium">{domain}</span>
        <span aria-hidden="true" className="ml-auto text-xs text-slate-500">
          ▾
        </span>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={6}
          className={MENU_CLASSES}
        >
          <DropdownMenu.RadioGroup
            value={domain}
            onValueChange={(value) => {
              if (value !== domain) {
                void navigate(domainRoute(value));
              }
            }}
          >
            {domains.map((entry) => (
              <DropdownMenu.RadioItem
                key={entry.name}
                value={entry.name}
                className={ITEM_CLASSES}
              >
                <DropdownMenu.ItemIndicator>
                  <span aria-hidden="true">*</span>
                </DropdownMenu.ItemIndicator>
                <span className="truncate">{entry.name}</span>
                {entry.engrams !== null && (
                  <span className="ml-auto text-xs text-slate-500 tabular-nums dark:text-slate-400">
                    {entry.engrams}
                  </span>
                )}
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/**
 * One folder: its subfolders, then its engrams.
 *
 * Folders first because they are the shape of the domain and the engrams are
 * what is in it, and a reader scanning for either finds the two in the same
 * place at every level.
 */
function TreeBranch({
  domain,
  path,
  permalink,
  browsing,
  marked,
}: {
  domain: string;
  path: string;
  permalink: string;
  /** The folder the screen beside the tree is pointed at, for what opens. */
  browsing: string;
  /** The folder that may read as the current page, for what is marked. */
  marked: string;
}) {
  const tree = useQuery(treeQuery(domain, path));

  if (tree.isPending) {
    return (
      <p className="px-2 py-1 text-sm text-slate-500 dark:text-slate-400">
        Loading engrams
      </p>
    );
  }
  if (tree.error) {
    return (
      <p
        role="alert"
        className="rounded bg-red-50 px-2 py-1.5 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
      >
        {problemDetail(tree.error)}
      </p>
    );
  }

  const folders = tree.data?.folders ?? [];
  const engrams = (tree.data?.engrams ?? []).filter(
    (row) => !isPinnedManifest(row),
  );
  if (folders.length === 0 && engrams.length === 0) {
    return (
      <p className="px-2 py-1 text-sm text-slate-500 dark:text-slate-400">
        {/*
          One sentence, and no instruction attached to it: the launcher the
          second half used to name is drawn only while an engram is open (see
          below), so on the domain's own home screen - which is exactly where
          an empty tree is read - it pointed at a control that was not there.
        */}
        {path === "" ? "This domain has no engrams yet." : "Nothing in here."}
      </p>
    );
  }

  return (
    <ul className="flex flex-col gap-0.5">
      {folders.map((name) => (
        <li key={`folder:${name}`}>
          <Folder
            domain={domain}
            path={childPath(path, name)}
            name={name}
            permalink={permalink}
            browsing={browsing}
            marked={marked}
          />
        </li>
      ))}
      {engrams.map((row) => (
        <li key={row.permalink}>
          <EngramLink row={row} current={row.permalink === permalink} />
        </li>
      ))}
      {tree.data?.truncated === true && (
        <li>
          <BrowseAll domain={domain} path={path} />
        </li>
      )}
    </ul>
  );
}

/**
 * The last row of a level the server had to cut: the whole folder, on the
 * screen that pages it.
 *
 * It carries no number, deliberately. This is a link, so its words are a
 * promise about where it goes, and the only count in hand is the wrong one for
 * that promise: the level knows how many engrams sit DIRECTLY in this folder,
 * while the screen it opens lists the folder and everything under it and
 * reports that larger total. Two numbers for one click is worse than none, and
 * at the root - where the row names the domain - a direct count reads as the
 * size of the whole domain, which it is not. The payload's `total` stays a
 * true fact about the level for anything that wants it; if a number is ever
 * wanted here it has to be the recursive one, which is a server field this
 * does not have.
 *
 * Muted and one step down in size, because it is the only row here that is not
 * a thing in the domain. Muted at slate-600 rather than the slate-500 the
 * app's captions usually wear, because this caption has a hover state under
 * it: slate-500 on the slate-100 wash is 4.35:1, under the 4.5:1 floor for
 * text this size, while slate-600 is 7.58:1 on white and 6.92:1 on the wash.
 * Dark needs no such step: slate-400 is 7.66:1 on slate-950 and 5.56:1 on the
 * slate-800 wash.
 */
function BrowseAll({ domain, path }: { domain: string; path: string }) {
  const here = path.split("/").pop() ?? path;
  return (
    <Link
      to={folderRoute(domain, path)}
      className={`block truncate rounded px-2 py-1 text-caption text-slate-600 hover:bg-slate-100 dark:text-slate-400 dark:hover:bg-slate-800 ${FOCUS_RING}`}
    >
      {path === ""
        ? "Browse all engrams in this domain"
        : `Browse all of ${here}`}
    </Link>
  );
}

/**
 * Whether a browse row is the domain's MANIFEST, which is pinned above the
 * tree and so must not be drawn inside it as well.
 *
 * The engine lists the manifest among a domain's engrams like any other file,
 * and its permalink is either the reserved name itself or whatever the file's
 * own frontmatter declares - `MANIFEST` on one domain, `manifest` on the next.
 * Case-insensitive covers both, and only a row at the domain's root can match:
 * a `notes/manifest` is somebody's own engram and stays in the tree.
 */
function isPinnedManifest(row: EngramRow): boolean {
  return row.permalink.toUpperCase() === "MANIFEST";
}

/**
 * A folder: a name that goes there, and a chevron that looks inside.
 *
 * The two are separate controls because they do two different things. The name
 * is a link to this folder on the domain screen, which is the surface that can
 * page a folder of any size; the chevron is a disclosure that opens the branch
 * here and leaves the screen beside it where the reader left it. What is
 * inside is mounted only while it is open, which is what makes the fetch lazy.
 *
 * The chevron is named after what it will do - "Expand notes", "Collapse
 * notes" - rather than after the folder, for two reasons: two controls in one
 * row both called `notes` would be indistinguishable to anybody listening
 * rather than looking, and the name then says the thing `aria-expanded` only
 * implies. `aria-controls` is left off: the region it would name does not
 * exist while the branch is shut, and an id pointing at nothing is worse than
 * a state the button already carries.
 *
 * A folder opens itself when the engram being read moves into it, or when the
 * screen beside it starts browsing it, whether that happened on arrival or by
 * a link somewhere else. It never closes itself: a reader who folded this
 * branch away meant it, and the state they set is theirs until they leave the
 * folder again.
 */
function Folder({
  domain,
  path,
  name,
  permalink,
  browsing,
  marked,
}: {
  domain: string;
  path: string;
  name: string;
  permalink: string;
  browsing: string;
  marked: string;
}) {
  // What opens and what is marked are two questions with two answers. The
  // branch opens when the reader is anywhere inside it - reading an engram
  // under it, or pointed at it by the screen beside the tree, filter or no
  // filter. It says "you are here" only when this folder really is the page
  // on screen, which is what `marked` already decided.
  const here = marked === path;
  const holdsCurrent =
    permalink.startsWith(`${path}/`) ||
    browsing === path ||
    browsing.startsWith(`${path}/`);
  const [open, setOpen] = useState(holdsCurrent);
  // Adjusted while rendering rather than in an effect, which is what React
  // asks for when state has to follow a prop: the branch opens in the same
  // pass that noticed the reader moved into it, with no flash of a shut
  // folder in between.
  const [held, setHeld] = useState(holdsCurrent);
  if (held !== holdsCurrent) {
    setHeld(holdsCurrent);
    if (holdsCurrent) {
      setOpen(true);
    }
  }

  return (
    <>
      {/*
        The row's own metrics, split across its two controls rather than
        dropped: `py-1` on both and the `px-2` shared between them, so a folder
        row is exactly as tall and as wide as the engram rows under it. The
        hover wash sits on the row rather than on either control, because what
        lights up under the pointer is the row.
      */}
      <div
        className={`flex items-center rounded text-sm hover:bg-slate-100 dark:hover:bg-slate-800 ${
          here ? "bg-slate-100 font-medium dark:bg-slate-800" : ""
        }`}
      >
        <button
          type="button"
          aria-expanded={open}
          aria-label={`${open ? "Collapse" : "Expand"} ${name}`}
          onClick={() => {
            setOpen((wasOpen) => !wasOpen);
          }}
          className={`rounded py-1 pr-1 pl-2 text-slate-500 dark:text-slate-400 ${FOCUS_RING}`}
        >
          <span aria-hidden="true" className="block w-3 text-xs">
            {open ? "▾" : "▸"}
          </span>
        </button>
        <Link
          to={folderRoute(domain, path)}
          aria-current={here ? "page" : undefined}
          className={`flex min-w-0 grow items-center gap-1.5 rounded py-1 pr-2 ${FOCUS_RING}`}
        >
          {/*
            Decorative: the row says `notes`, and the icon is what makes a
            folder legible as a folder at a glance rather than a second name
            for it. The engram rows stay icon-free on purpose - the icon IS
            the distinction, and one on every row would say nothing.
          */}
          <FolderIcon
            aria-hidden="true"
            size={16}
            strokeWidth={1.75}
            className="shrink-0"
          />
          <span className="truncate">{name}</span>
        </Link>
      </div>
      {open && (
        <div className="ml-3 border-l border-slate-200 pl-1 dark:border-slate-800">
          <TreeBranch
            domain={domain}
            path={path}
            permalink={permalink}
            browsing={browsing}
            marked={marked}
          />
        </div>
      )}
    </>
  );
}

/**
 * One engram in the tree.
 *
 * Named by its title alone: this is navigation, and the row that says what an
 * engram is made of is the list on the screen beside it. A retired one is
 * faded and still there, which is the rule everywhere else in this app.
 */
function EngramLink({ row, current }: { row: EngramRow; current: boolean }) {
  const retired = isRetired(row.status);
  return (
    <Link
      to={engramRoute(row.domain, row.permalink)}
      aria-current={current ? "page" : undefined}
      className={`block truncate rounded px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:hover:bg-slate-800 ${
        current ? "bg-slate-100 font-medium dark:bg-slate-800" : ""
      } ${retired ? RETIRED_CLASS : ""}`}
    >
      {row.title}
    </Link>
  );
}

/** The path of a subfolder, from the folder it sits in. */
function childPath(path: string, name: string): string {
  return path === "" ? name : `${path}/${name}`;
}
