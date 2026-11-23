/**
 * The Administration area at `/admin`: the frame around every page an admin
 * uses to look after this instance, and the small pages that have no screen
 * of their own yet.
 *
 * A side navigation of links beside the page at wide widths, and a select
 * above it at drawer width, the way a domain's settings index its sections.
 * The current page is the link with `aria-current="page"`. Maintenance is
 * linked from here but stays at `/maintenance`, because every role may open
 * it.
 *
 * Only an admin sees any of it. Anyone else gets the not-found screen at
 * every address under `/admin`, the answer the users page always gave, so
 * the area says no more than a mistyped address does. The pages inside are
 * never mounted for them, so nothing is asked of the server either.
 */

import { ExternalLink } from "lucide-react";
import type { ReactNode } from "react";
import { Link, Outlet, useLocation, useNavigate } from "react-router";

import { useAuth } from "../auth/AuthContext";
import { releaseNotesUrl, UPDATE_COMMAND } from "../auth/updateNote";
import { absoluteUrl } from "../base";
import { FIELD, FOCUS_RING, RAIL_LABEL, SIDE } from "../components/primitives";
import { copyText, useSaid } from "../components/useSaid";
import {
  adminAgentsRoute,
  adminGeneralRoute,
  adminInvitesRoute,
  githubSettingsRoute,
  maintenanceRoute,
  usersRoute,
} from "../paths";
import { PREFETCH } from "../prefetch";
import { usePageTitle } from "../usePageTitle";
import NotFound from "./NotFound";

/** One page of the area, in the order the navigation lists them. */
interface AdminPage {
  readonly to: string;
  readonly label: string;
  readonly warm?: { onPointerEnter: () => void; onFocus: () => void };
}

const PAGES: readonly AdminPage[] = [
  { to: adminGeneralRoute(), label: "General" },
  { to: usersRoute(), label: "Users", warm: PREFETCH.users },
  { to: adminAgentsRoute(), label: "Agents" },
  { to: adminInvitesRoute(), label: "Invites" },
  {
    to: githubSettingsRoute(),
    label: "GitHub connection",
    warm: PREFETCH.github,
  },
];

export default function AdminArea() {
  const { capabilities } = useAuth();
  if (!capabilities.canAdminister) {
    return <NotFound />;
  }
  return <AdminFrame />;
}

function AdminFrame() {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const current = PAGES.find((page) => page.to === pathname) ?? null;
  return (
    <div className="flex flex-col gap-6 lg:grid lg:grid-cols-[11rem_minmax(0,1fr)] lg:gap-14">
      <div className="flex items-center gap-4 lg:hidden">
        <label htmlFor="admin-page" className="sr-only">
          Administration page
        </label>
        <select
          id="admin-page"
          value={current?.to ?? ""}
          onChange={(event) => {
            void navigate(event.target.value);
          }}
          className={`min-w-0 flex-1 ${FIELD}`}
        >
          {current === null && <option value="">Administration</option>}
          {PAGES.map((page) => (
            <option key={page.to} value={page.to}>
              {page.label}
            </option>
          ))}
        </select>
        <Link
          to={maintenanceRoute()}
          {...PREFETCH.maintenance}
          className={`shrink-0 rounded text-sm text-link underline-offset-2 hover:underline ${FOCUS_RING}`}
        >
          Maintenance
        </Link>
      </div>
      <nav
        aria-label="Administration"
        className="sticky top-20 hidden self-start lg:block"
      >
        <p className={`mb-2 px-2.5 ${RAIL_LABEL}`}>Administration</p>
        <ul className="flex flex-col gap-0.5">
          {PAGES.map((page) => {
            const here = page === current;
            return (
              <li key={page.to}>
                <Link
                  to={page.to}
                  {...page.warm}
                  aria-current={here ? "page" : undefined}
                  className={`${SIDE.row} ${here ? SIDE.current : ""}`}
                >
                  {page.label}
                </Link>
              </li>
            );
          })}
        </ul>
        <p className={`mt-6 mb-2 px-2.5 ${RAIL_LABEL}`}>For every role</p>
        <ul className="flex flex-col gap-0.5">
          <li>
            <Link
              to={maintenanceRoute()}
              {...PREFETCH.maintenance}
              className={SIDE.row}
            >
              Maintenance
            </Link>
          </li>
        </ul>
      </nav>
      <div className="min-w-0">
        <Outlet />
      </div>
    </div>
  );
}

/** A page heading with the one line under it that says what the page is for. */
function PageHeader({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <header>
      <h1 className="text-display">{title}</h1>
      <p className="mt-1 text-sm text-muted">{children}</p>
    </header>
  );
}

/** One fact on the General page: a term and what it is on this instance. */
function Fact({ term, children }: { term: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1 py-4 first:pt-0 sm:flex-row sm:gap-6">
      <dt className="w-40 shrink-0 text-sm font-medium text-ink">{term}</dt>
      <dd className="min-w-0 text-sm text-ink-3">{children}</dd>
    </div>
  );
}

/**
 * The area's first page: which version this server runs, whether its update
 * check found a newer one, and the address this instance is reached at. All
 * of it comes from the probe the app already made, so the page asks the
 * server nothing.
 */
export function AdminGeneral() {
  usePageTitle("Administration");
  const { capabilities } = useAuth();
  const [said, say] = useSaid();
  const address = absoluteUrl("/");
  const available = capabilities.updateAvailable;
  return (
    <div className="flex max-w-3xl flex-col gap-6">
      <PageHeader title="General">
        This instance at a glance. The users, agents, invites and the GitHub
        connection have pages of their own.
      </PageHeader>
      <dl className="flex flex-col divide-y divide-hair">
        <Fact term="Version">
          {capabilities.serverVersion === ""
            ? "The server did not say."
            : `Crystalline ${capabilities.serverVersion}`}
        </Fact>
        <Fact term="Updates">
          {available === null ? (
            <p>
              No newer release is known. The update check runs on the server,
              when update.check is on.
            </p>
          ) : (
            <div className="flex flex-col gap-2">
              <p>{available} is available.</p>
              <a
                href={releaseNotesUrl(available)}
                target="_blank"
                rel="noopener noreferrer"
                className={`inline-flex items-center gap-1 self-start rounded text-link underline-offset-2 hover:underline ${FOCUS_RING}`}
              >
                Release notes{" "}
                <span className="sr-only">(opens in a new tab)</span>
                <ExternalLink aria-hidden="true" size={13} strokeWidth={1.75} />
              </a>
              <p>
                Run on the server:{" "}
                <code className="rounded bg-sunken px-1.5 py-0.5 font-mono text-[12.5px] text-ink">
                  {UPDATE_COMMAND}
                </code>
              </p>
            </div>
          )}
        </Fact>
        <Fact term="Address">
          <div className="flex flex-wrap items-center gap-2">
            <code className="min-w-0 rounded bg-sunken px-1.5 py-0.5 font-mono text-[12.5px] break-all text-ink">
              {address}
            </code>
            <button
              type="button"
              aria-label="Copy the address"
              onClick={() => {
                copyText(address, say);
              }}
              className={`rounded px-1 text-caption text-ink-3 hover:text-ink ${FOCUS_RING}`}
            >
              Copy
            </button>
            <span role="status" className="text-caption text-ink-3">
              {said}
            </span>
          </div>
          <p className="mt-1 text-caption text-muted">
            The address this browser opened Fluid at.
          </p>
        </Fact>
      </dl>
    </div>
  );
}

/** The agents page, a placeholder until its screen lands later in this release. */
export function AdminAgents() {
  usePageTitle("Agents");
  return (
    <PageHeader title="Agents">
      The agents of this instance get their page here later in this release.
    </PageHeader>
  );
}

/** The invites page, a placeholder until its screen lands later in this release. */
export function AdminInvites() {
  usePageTitle("Invites");
  return (
    <PageHeader title="Invites">
      The invite links of this instance get their page here later in this
      release.
    </PageHeader>
  );
}
