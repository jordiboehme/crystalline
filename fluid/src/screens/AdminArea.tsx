/**
 * The Administration area at `/admin`: the frame around every page an admin
 * uses to look after this instance, and its General page.
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
import {
  releaseNotesUrl,
  UPDATE_AUTO_ON_COMMAND,
  UPDATE_COMMAND,
} from "../auth/updateNote";
import type { UpdateCheckOff, UpdateCheckState } from "../auth/updateNote";
import { absoluteUrl } from "../base";
import { relativeTime } from "../format";
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
import { PageHeader } from "./AdminParts";
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
  // The page itself or any address inside it (an account at
  // `/admin/users/<name>`, or a trailing slash).
  const current =
    PAGES.find(
      (page) => pathname === page.to || pathname.startsWith(`${page.to}/`),
    ) ?? null;
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

/** One fact on the General page: a term and what it is on this instance. */
function Fact({ term, children }: { term: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1 py-4 first:pt-0 sm:flex-row sm:gap-6">
      <dt className="w-40 shrink-0 text-sm font-medium text-ink">{term}</dt>
      <dd className="min-w-0 text-sm text-ink-3">{children}</dd>
    </div>
  );
}

/** Code in a sentence, the way the page writes a command or a file. */
function Command({ children }: { children: ReactNode }) {
  return (
    <code className="rounded bg-sunken px-1.5 py-0.5 font-mono text-[12.5px] break-all text-ink">
      {children}
    </code>
  );
}

/** Why a check that is off is off, and what to do instead, per reason. */
const OFF_TEXT: Record<Exclude<UpdateCheckOff, "not_turned_on">, string> = {
  no_update_notifier:
    "Update check off: NO_UPDATE_NOTIFIER is set on the server. It keeps the check off whatever update.check says. Once it is removed, the check follows update.check: crystalline update auto on turns it on.",
  ci: "Update check off: CI is set on the server. It keeps the check off whatever update.check says. Once it is removed, the check follows update.check: crystalline update auto on turns it on.",
  docker:
    "Update check off: this server runs in Docker. To update, pull the new image and start the container again.",
  desktop_extension:
    "Update check off: this is the Claude Desktop-only bundle. To update, open the newer .mcpb file by hand.",
  no_state_folder:
    "Update check off: the server has no state folder to keep a check in.",
};

/**
 * Whether the server's daily update check is on, what its last run found,
 * and, when it is off, why and what to do. Read-only: the check is turned on
 * on the server, so a check that is merely not turned on gets the command
 * to copy, not a switch. `available` says a release is offered, which the
 * row names below this text.
 */
function UpdateCheckText({
  check,
  available,
}: {
  check: UpdateCheckState | null;
  available: boolean;
}) {
  const [said, say] = useSaid();
  if (check === null) {
    return available ? null : (
      <p>The server did not say whether its update check is on.</p>
    );
  }
  if (check.offReason === "not_turned_on") {
    return (
      <div className="flex flex-col gap-1">
        <p>Update check off.</p>
        <div className="flex flex-wrap items-center gap-2">
          <span>Turn it on on the server:</span>
          <Command>{UPDATE_AUTO_ON_COMMAND}</Command>
          <button
            type="button"
            aria-label="Copy the command"
            onClick={() => {
              copyText(UPDATE_AUTO_ON_COMMAND, say);
            }}
            className={`rounded px-1 text-caption text-ink-3 hover:text-ink ${FOCUS_RING}`}
          >
            Copy
          </button>
          <span role="status" className="text-caption text-ink-3">
            {said}
          </span>
        </div>
        {check.deb && (
          <p className="text-caption text-muted">
            The crystalline service of a .deb install reads its settings from{" "}
            <Command>/etc/default/crystalline</Command>: set{" "}
            <Command>CRYSTALLINE_UPDATE_CHECK=true</Command> there, then run:{" "}
            <Command>sudo systemctl restart crystalline</Command>
          </p>
        )}
      </div>
    );
  }
  if (check.offReason !== null) {
    return <p>{OFF_TEXT[check.offReason]}</p>;
  }
  const when =
    check.lastChecked === null
      ? null
      : relativeTime(check.lastChecked, new Date());
  if (when === null) {
    return <p>Update check on. It has not checked yet.</p>;
  }
  // A result the page does not know says only when the check ran.
  const found: ReactNode =
    check.lastCheck === null ? null : check.lastCheck === "failed" ? (
      " The check failed."
    ) : check.lastCheck === "up_to_date" ? (
      " No newer release."
    ) : available ? null : (
      <>
        {" "}
        A newer release is out, but this server does not offer it yet. To see
        why, run on the server: <Command>crystalline status</Command>
      </>
    );
  return (
    <p>
      Update check on. Last checked {when}.{found}
    </p>
  );
}

/**
 * The area's first page: which version this server runs, whether its update
 * check is on and found a newer one, and the address this instance is
 * reached at. All of it comes from the probe the app already made, so the
 * page asks the server nothing.
 */
export function AdminGeneral() {
  usePageTitle("Administration");
  const { capabilities } = useAuth();
  const [said, say] = useSaid();
  const address = absoluteUrl("/");
  const available = capabilities.updateAvailable;
  const check = capabilities.updateCheck;
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
          <div className="flex flex-col gap-2">
            <UpdateCheckText check={check} available={available !== null} />
            {available !== null && (
              <>
                <p>{available} is available.</p>
                <a
                  href={releaseNotesUrl(available)}
                  target="_blank"
                  rel="noopener noreferrer"
                  className={`inline-flex items-center gap-1 self-start rounded text-link underline-offset-2 hover:underline ${FOCUS_RING}`}
                >
                  Release notes{" "}
                  <span className="sr-only">(opens in a new tab)</span>
                  <ExternalLink
                    aria-hidden="true"
                    size={13}
                    strokeWidth={1.75}
                  />
                </a>
                <p>
                  Run on the server:{" "}
                  <code className="rounded bg-sunken px-1.5 py-0.5 font-mono text-[12.5px] text-ink">
                    {UPDATE_COMMAND}
                  </code>
                </p>
              </>
            )}
          </div>
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
