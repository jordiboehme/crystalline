/**
 * The admin surface: the GitHub connection, the domain lifecycle, a team
 * domain's sync state and the archive round trip.
 *
 * Constants, types and fetchers only - no components - so a screen importing a
 * query key does not drag a module fast refresh has to give up on.
 *
 * Two reading styles live here, and which one applies is decided by the
 * document rather than by taste: the GitHub status and the archive report are
 * real schemas, so they are typed from the generated `components` through the
 * aliases in `model.ts`, while the domain registration report, the
 * unregistration receipt and the sync status are the engine's own JSON passed
 * through unchanged (the API answers with one payload rather than two shapes
 * that drift), so they are read field by field the way `domains.ts` reads its
 * listing. Everything a screen sees is camelCase; the snake_case stops here.
 */

import { API_BASE, api, encodeSegment } from "./client";
import { asArray, asNumber, asObject, asString } from "./json";
import type {
  ArchiveReport as ArchiveReportWire,
  CreateDomainWireBody,
  GithubStatusResponse,
} from "./model";

/**
 * The bytes of an archive are announced as what they are.
 *
 * `client.ts` only defaults an UNSET content type to `application/json`, so
 * this header is what keeps a zip from being announced as JSON - and the
 * server refuses anything else on these two routes.
 */
const ZIP_HEADERS = { "Content-Type": "application/zip" };

/** The half of a running device flow a browser has to show. */
export interface GithubPending {
  /** The short code the user types in at {@link GithubPending.verificationUrl}. */
  userCode: string;
  /** Where the user confirms the code. */
  verificationUrl: string;
  /** How many seconds from the flow's start the code stays valid. */
  expiresInSecs: number;
}

/**
 * The GitHub connection, as the settings screen renders and polls it. No token
 * material, ever: only whether the feature is on, whether a credential is on
 * file, whose it is and where it lives.
 */
export interface GithubStatus {
  /** Whether the feature is on at all: team tools and origin polling. */
  enabled: boolean;
  /** Whether a credential is on file for this instance. */
  connected: boolean;
  /** The account login, when connected. */
  user: string | null;
  /** `keyring`, `file` or `environment`; null when disconnected. */
  tokenStore: string | null;
  /** A device flow waiting for the browser side, or null when none runs. */
  pending: GithubPending | null;
  /**
   * A finished flow's failure, in the server's words. Reported on exactly one
   * status read after the flow ends, then cleared.
   */
  error: string | null;
}

/** The cache key of the GitHub connection. */
export const GITHUB_STATUS_KEY = ["settings", "github"] as const;

/**
 * Read a status payload, whatever arrived.
 *
 * Defensive rather than cast even though the schema is real, because this one
 * is also the device flow's poll: it is read on a timer while a background
 * flow finishes, and a field that is briefly missing should leave the screen
 * saying "not connected" rather than throwing inside a render.
 */
export function readGithubStatus(payload: unknown): GithubStatus {
  const record = asObject(payload);
  return {
    enabled: record?.enabled === true,
    connected: record?.connected === true,
    user: asString(record?.user),
    tokenStore: asString(record?.token_store),
    pending: readPending(record?.pending),
    error: asString(record?.error),
  };
}

/** The pending block, or null when there is no flow to show. */
function readPending(value: unknown): GithubPending | null {
  const record = asObject(value);
  const userCode = asString(record?.user_code);
  const verificationUrl = asString(record?.verification_url);
  // Both halves or nothing: a code with nowhere to type it, or a place with no
  // code, is not something to put in front of somebody.
  if (userCode === null || verificationUrl === null) {
    return null;
  }
  return {
    userCode,
    verificationUrl,
    expiresInSecs: asNumber(record?.expires_in_secs) ?? 0,
  };
}

/** The connection as it stands. Also the device flow's poll. */
export async function fetchGithubStatus(): Promise<GithubStatus> {
  return readGithubStatus(await api<GithubStatusResponse>("/settings/github"));
}

/** Start a device-code sign-in, or report the one already running. */
export async function startGithubConnect(): Promise<GithubStatus> {
  return readGithubStatus(
    await api<GithubStatusResponse>("/settings/github/connect", {
      method: "POST",
    }),
  );
}

/**
 * Connect with a personal access token.
 *
 * The token goes out in this one body and is held nowhere: the answer is the
 * same token-material-free status every other verb here returns, and the
 * caller's own copy is a field of a form that clears itself.
 */
export async function submitGithubToken(token: string): Promise<GithubStatus> {
  return readGithubStatus(
    await api<GithubStatusResponse>("/settings/github/token", {
      method: "POST",
      body: JSON.stringify({ token }),
    }),
  );
}

/** Forget the stored credential. Leaves the feature itself switched on. */
export async function disconnectGithub(): Promise<GithubStatus> {
  return readGithubStatus(
    await api<GithubStatusResponse>("/settings/github", { method: "DELETE" }),
  );
}

/** Which of the three kinds of domain is being registered. */
export type DomainMode = "local" | "virtual" | "github";

/**
 * A domain to register. Every field but `mode` belongs to one of the modes:
 * a local or virtual domain is named, a team domain names a repository.
 */
export interface CreateDomainBody {
  mode: DomainMode;
  name?: string;
  repo?: string;
  branch?: string;
  path?: string;
}

/** What a registration reports back: the name it took, and where it landed. */
export interface CreatedDomain {
  domain: string;
  /** The folder a local domain was created in; null for a virtual one. */
  root: string | null;
}

/** Register a domain: a local folder, a virtual one, or a GitHub team domain. */
export async function createDomain(
  body: CreateDomainBody,
): Promise<CreatedDomain> {
  // Checked against the generated schema on the way out, so a field this app
  // invents cannot reach a server that would refuse it.
  const wire: CreateDomainWireBody = body;
  const report = asObject(
    await api<unknown>("/domains", {
      method: "POST",
      body: JSON.stringify(wire),
    }),
  );
  return {
    // The report names the domain, including the one case the request did not:
    // a team domain defaults its name to the repository's.
    domain: asString(report?.domain) ?? body.name ?? "",
    root: asString(report?.root),
  };
}

/** What an unregistration leaves behind, so a screen can say it out loud. */
export interface UnregisterReceipt {
  /** Whether the domain's files are still on disk. False for a virtual one. */
  filesKept: boolean;
  /** How many co-editing rooms were saved and closed on the way out. */
  roomsClosed: number;
}

/** Unregister a domain. Files on disk are never touched. */
export async function unregisterDomain(
  name: string,
): Promise<UnregisterReceipt> {
  const report = asObject(
    await api<unknown>(`/domains/${encodeSegment(name)}`, {
      method: "DELETE",
    }),
  );
  return {
    filesKept: report?.files_kept === true,
    roomsClosed: asNumber(report?.rooms_closed) ?? 0,
  };
}

/** Where a team domain stands relative to its GitHub origin. */
export interface SyncStatus {
  repo: string;
  branch: string | null;
  lastChecked: string | null;
  /** Unshared local work, as a count. */
  localChanges: number;
  /** Proposals still open on the origin, as a count. */
  openProposals: number;
  /** Proposals the team turned down, as a count. */
  declinedProposals: number;
  /** Files a pull could not merge and somebody has to settle, as a count. */
  conflicts: number;
  /** Whether the origin is ahead, or null when the probe could not say. */
  behind: boolean | null;
  /**
   * Why the live origin check failed, in the server's own words, or null when
   * it succeeded or was never attempted.
   *
   * The engine answers this call rather than failing it when the probe cannot
   * reach GitHub - offline, rate limited, an expired connection - by retrying
   * with no probe at all, so every other field above is then local state
   * alone: true about this copy, and possibly days behind the origin. A card
   * that shows those numbers without showing this shows stale facts as fresh.
   */
  probeError: string | null;
}

/** The cache key of one domain's sync status. */
export function syncStatusKey(domain: string): readonly unknown[] {
  return ["domains", domain, "sync"];
}

/**
 * A count that may arrive as a number or as the list it counts.
 *
 * The engine's status report embeds the proposals and the conflicts themselves
 * while its poll overview counts them, and both spellings reach this surface. A
 * card wants the number either way.
 */
function asCount(value: unknown): number {
  return asNumber(value) ?? asArray(value).length;
}

/** Read a sync status out of the engine's own per-domain report. */
function readSyncStatus(payload: unknown): SyncStatus {
  const record = asObject(payload);
  return {
    repo: asString(record?.repo) ?? "",
    branch: asString(record?.branch),
    lastChecked: asString(record?.last_checked),
    localChanges: asCount(record?.local_changes),
    openProposals: asCount(record?.open_proposals),
    declinedProposals: asCount(record?.declined_proposals),
    conflicts: asCount(record?.conflicts),
    behind: typeof record?.behind === "boolean" ? record.behind : null,
    probeError: asString(record?.probe_error),
  };
}

/** Where this team domain stands. 404 for a domain with no origin. */
export async function fetchSyncStatus(domain: string): Promise<SyncStatus> {
  return readSyncStatus(
    await api<unknown>(`/domains/${encodeSegment(domain)}/sync`),
  );
}

/** Pull this team domain's origin now. */
export async function syncDomain(domain: string): Promise<unknown> {
  return api<unknown>(`/domains/${encodeSegment(domain)}/sync`, {
    method: "POST",
  });
}

/** One verify finding raised over an archived entry's markdown. */
export interface ArchiveFinding {
  rule: string;
  severity: string;
  message: string;
  line: number | null;
}

/** One entry of an uploaded archive, and what became of it. */
export interface ArchiveEntry {
  /** The entry's path inside the archive, domain-relative. */
  path: string;
  /** preview: new | collides | invalid | ignored. import: created | overwritten | skipped | invalid | ignored. */
  status: string;
  permalink: string | null;
  /** Why the entry was not written, in the words of whatever refused it. */
  reason: string | null;
  findings: ArchiveFinding[];
}

/** The per-entry report of a preview or an import, with its counters. */
export interface ArchiveReport {
  entries: ArchiveEntry[];
  written: number;
  skipped: number;
  invalid: number;
  ignored: number;
}

/**
 * Where a domain's archive is downloaded from.
 *
 * An address rather than a fetch: the download is a cookie-authenticated GET
 * that an anchor performs, so the browser saves the file itself instead of the
 * app holding a whole archive in memory to hand back to it.
 */
export function archiveDownloadUrl(domain: string): string {
  return `${API_BASE}/domains/${encodeSegment(domain)}/archive`;
}

/** The counters and entries a screen shows, out of the generated shape. */
function readArchiveReport(report: ArchiveReportWire): ArchiveReport {
  return {
    entries: report.entries.map((entry) => ({
      path: entry.path,
      status: entry.status,
      permalink: entry.permalink ?? null,
      reason: entry.reason ?? null,
      findings: entry.findings.map((finding) => ({
        rule: finding.rule,
        severity: finding.severity,
        message: finding.message,
        line: finding.line ?? null,
      })),
    })),
    written: report.written,
    skipped: report.skipped,
    invalid: report.invalid,
    ignored: report.ignored,
  };
}

/** Dry-run an archive upload: what each entry would become. Writes nothing. */
export async function previewArchive(
  domain: string,
  data: ArrayBuffer,
): Promise<ArchiveReport> {
  return readArchiveReport(
    await api<ArchiveReportWire>(
      `/domains/${encodeSegment(domain)}/archive/preview`,
      { method: "POST", body: data, headers: ZIP_HEADERS },
    ),
  );
}

/** Import an archive, running the verb the preview dry-ran. */
export async function importArchive(
  domain: string,
  data: ArrayBuffer,
  policy: "skip" | "overwrite",
): Promise<ArchiveReport> {
  // `skip` is the server's own default, so it is left unsaid: the query
  // parameter appears exactly when somebody chose the other thing.
  const query = policy === "overwrite" ? "?policy=overwrite" : "";
  return readArchiveReport(
    await api<ArchiveReportWire>(
      `/domains/${encodeSegment(domain)}/archive/import${query}`,
      { method: "POST", body: data, headers: ZIP_HEADERS },
    ),
  );
}
