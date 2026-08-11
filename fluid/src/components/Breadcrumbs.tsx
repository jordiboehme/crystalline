/**
 * The path row under the top bar: where this document lives, one crumb per
 * folder. Folder crumbs are plain text - no folder route exists to point at -
 * and the leaf names the page itself. Middle crumbs hide on narrow viewports
 * so a deep path degrades to "domain > title" rather than wrapping.
 */

import { ChevronRight } from "lucide-react";
import type { ReactElement } from "react";
import { Link } from "react-router";

import { domainRoute } from "../paths";
import { FOCUS_RING } from "./primitives";

export interface Crumb {
  label: string;
  href: string | null;
}

// eslint-disable-next-line react-refresh/only-export-components
export function crumbsOf(
  domain: string,
  permalink: string,
  title: string,
): Crumb[] {
  const segments = permalink.split("/").filter((segment) => segment !== "");
  const folders = segments.slice(0, -1);
  return [
    { label: domain, href: domainRoute(domain) },
    ...folders.map((folder) => ({ label: folder, href: null })),
    { label: title, href: null },
  ];
}

export function Breadcrumbs({ crumbs }: { crumbs: Crumb[] }): ReactElement {
  const last = crumbs.length - 1;
  return (
    // Deliberately NOT print:hidden: on paper the details panel is hidden,
    // so the trail is the one line that still says where this document
    // lives (domain, folders, title) - the printed address.
    <nav aria-label="Breadcrumb" className="min-w-0">
      <ol className="flex min-w-0 flex-wrap items-center gap-1 text-caption text-slate-500 dark:text-slate-400">
        {crumbs.map((crumb, at) => (
          <li
            key={`${String(at)}-${crumb.label}`}
            className={`flex min-w-0 items-center gap-1 ${
              at !== 0 && at !== last ? "hidden sm:flex" : ""
            }`}
          >
            {at > 0 && (
              <ChevronRight aria-hidden="true" size={12} className="shrink-0" />
            )}
            {crumb.href !== null ? (
              <Link
                to={crumb.href}
                className={`max-w-48 truncate rounded hover:underline ${FOCUS_RING}`}
              >
                {crumb.label}
              </Link>
            ) : (
              <span
                aria-current={at === last ? "page" : undefined}
                className={`max-w-48 truncate ${
                  at === last
                    ? "font-medium text-slate-700 dark:text-slate-200"
                    : ""
                }`}
              >
                {crumb.label}
              </span>
            )}
          </li>
        ))}
      </ol>
    </nav>
  );
}
