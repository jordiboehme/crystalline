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
 * on a row nobody can see marks nothing.
 */

import { useQuery } from "@tanstack/react-query";
import { DropdownMenu } from "radix-ui";
import { useState } from "react";
import { Link, useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import { fetchTree, treeKey } from "../api/domain";
import type { DomainSummary } from "../api/domains";
import type { EngramRow } from "../api/engrams";
import { useAuth } from "../auth/AuthContext";
import { RETIRED_CLASS, isRetired } from "../lifecycle";
import { domainRoute, engramRoute, manifestRoute } from "../paths";
import { CreateEngramDialog } from "./CreateEngramDialog";
import { ITEM_CLASSES, MENU_CLASSES } from "./menu";

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

  return (
    <div className="flex flex-col gap-3">
      <Link
        to="/"
        className="px-2 text-xs text-slate-500 underline underline-offset-2 hover:no-underline focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:text-slate-400"
      >
        All domains
      </Link>

      <DomainSwitcher domain={domain} domains={domains} />

      {/*
        Pinned ahead of the tree and styled apart from it - a dashed border
        and mono caps rather than the engram rows' plain text - because a
        MANIFEST is not an engram: it is what introduces the domain, not
        something filed inside it. The you-are-here cue is `EngramLink`'s own
        mechanism, `aria-current` plus a highlight class, so a reader on
        either page gets the same signal regardless of which row it marks.
      */}
      <Link
        to={manifestRoute(domain)}
        aria-current={onManifest ? "page" : undefined}
        className={`mx-2 block truncate rounded border border-dashed border-slate-300 px-2 py-1 font-mono text-xs tracking-wide uppercase hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800 ${
          onManifest ? "bg-slate-100 font-medium dark:bg-slate-800" : ""
        }`}
      >
        MANIFEST
      </Link>

      <div>
        <h2 className="px-2 pb-2 text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">
          Engrams
        </h2>
        <TreeBranch domain={domain} path="" permalink={permalink} />
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
            className="mt-2 w-full rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
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
        className="flex w-full items-center gap-2 rounded border border-slate-300 px-2 py-1.5 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
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
}: {
  domain: string;
  path: string;
  permalink: string;
}) {
  const tree = useQuery({
    queryKey: treeKey(domain, path),
    queryFn: () => fetchTree(domain, path),
  });

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
  const engrams = tree.data?.engrams ?? [];
  if (folders.length === 0 && engrams.length === 0) {
    return (
      <p className="px-2 py-1 text-sm text-slate-500 dark:text-slate-400">
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
          />
        </li>
      ))}
      {engrams.map((row) => (
        <li key={row.permalink}>
          <EngramLink row={row} current={row.permalink === permalink} />
        </li>
      ))}
    </ul>
  );
}

/**
 * A folder, open or shut.
 *
 * A disclosure button rather than a link: opening a folder is a look inside
 * the sidebar, and the screen beside it stays where the reader left it. What
 * is inside is mounted only while it is open, which is what makes the fetch
 * lazy.
 *
 * A folder opens itself when the engram being read moves into it, whether that
 * happened on arrival or by a link somewhere else on the screen. It never
 * closes itself: a reader who folded this branch away meant it, and the state
 * they set is theirs until they leave the folder again.
 */
function Folder({
  domain,
  path,
  name,
  permalink,
}: {
  domain: string;
  path: string;
  name: string;
  permalink: string;
}) {
  const holdsCurrent = permalink.startsWith(`${path}/`);
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
      <button
        type="button"
        aria-expanded={open}
        onClick={() => {
          setOpen((wasOpen) => !wasOpen);
        }}
        className="flex w-full items-center gap-1 rounded px-2 py-1 text-left text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:hover:bg-slate-800"
      >
        <span
          aria-hidden="true"
          className="w-3 text-xs text-slate-500 dark:text-slate-400"
        >
          {open ? "▾" : "▸"}
        </span>
        <span className="truncate">{name}</span>
      </button>
      {open && (
        <div className="ml-3 border-l border-slate-200 pl-1 dark:border-slate-800">
          <TreeBranch domain={domain} path={path} permalink={permalink} />
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
      className={`block truncate rounded px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:hover:bg-slate-800 ${
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
