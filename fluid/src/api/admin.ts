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
import { asArray, asNumber, asObject, asString, asStrings } from "./json";
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

/** One comment or review left on a proposal, in the forge's own words. */
export interface ProposalFeedback {
  /** The commenting account's login. */
  author: string;
  body: string;
  /** The file an inline comment is anchored to; null for the other channels. */
  path: string | null;
  /** The line an inline comment is anchored to; null for the other channels. */
  line: number | null;
  submittedAt: string | null;
  /** `review`, `review_comment` or `comment`. */
  kind: string;
}

/** One proposal on the origin, whichever list it arrived in. */
export interface SyncProposal {
  number: number;
  url: string;
  title: string;
  /** Lowercased: `open`, `merged`, `declined` or `withdrawn`. */
  status: string;
  /** `approved`, `changes_requested` or `commented`; null when unreviewed. */
  reviewState: string | null;
  /** Whether a reviewer moved the proposal branch out from under this copy. */
  amendedUpstream: boolean;
  feedback: ProposalFeedback[];
  updatedAt: string | null;
}

/** One file a pull could not merge, as the status report lists it. */
export interface SyncConflict {
  id: string;
  path: string;
  kind: string;
  detectedAt: string | null;
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
  /**
   * Which kind of origin this is, in the server's own word, or null when the
   * report did not say. Only `github` exists today, and the route sets it
   * because a client looking at a sync card has to know what it is looking at
   * without inferring it from the fields that happen to be filled in.
   */
  mode: string | null;
  /**
   * Whether this instance has a GitHub credential on file, or null when the
   * report carried no connection block at all.
   *
   * The three states are three different sentences, which is why this is not a
   * boolean: `false` is "connect GitHub and this starts working", and it is an
   * answer the status route goes out of its way to give rather than refusing
   * over. `null` is "this report does not say", and a card that read it as
   * `false` would tell somebody to connect what is already connected.
   */
  connected: boolean | null;
  /**
   * The open and the declined proposals as themselves, in that order, for the
   * card that draws a row per proposal.
   *
   * One list rather than two, because a row says which it is: the status a
   * proposal carries is what tells an open one from a turned-down one, and
   * splitting them here would only make every screen join them again. Empty
   * when the report counted its proposals instead of embedding them, which is
   * what the counts above are read out of.
   */
  proposals: SyncProposal[];
  /**
   * The conflicts as themselves, for the screen that settles them. Empty when
   * the report carried a count, or a list of bare paths with no id to address
   * a conflict by.
   */
  conflictList: SyncConflict[];
}

/** The cache key of one domain's sync status. */
export function syncStatusKey(domain: string): readonly unknown[] {
  return ["domains", domain, "sync"];
}

/**
 * The cache key of one domain's share plan, and the one key in this app that
 * deliberately sits outside the `["domains", ...]` family.
 *
 * Reading a plan pulls the origin, so this is not a cache of domain content:
 * react-query invalidates by prefix, and a plan filed under `["domains"]`
 * would be refetched - which is to say, would pull - by every bulk domain
 * invalidation in the app. The share dialog's own comment carries the rest of
 * the reasoning, because that is where the invalidation it would collide with
 * is fired from.
 */
export function sharePlanKey(domain: string): readonly unknown[] {
  return ["share-plan", domain];
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

/**
 * One proposal, whichever list it came from.
 *
 * The number is the identity every write here is addressed by, so a record
 * without one is dropped rather than drawn as a row nothing can be done to.
 */
function readProposal(value: unknown): SyncProposal | null {
  const record = asObject(value);
  const number = asNumber(record?.number);
  if (number === null) {
    return null;
  }
  return {
    number,
    url: asString(record?.url) ?? "",
    title: asString(record?.title) ?? "",
    // The engine spells the three pre-existing statuses PascalCase and the
    // withdrawn one lowercase; fold them here so screens compare one casing.
    status: (asString(record?.status) ?? "open").toLowerCase(),
    reviewState: asString(record?.review_state),
    amendedUpstream: record?.amended_upstream === true,
    feedback: asArray(record?.feedback)
      .map(readFeedback)
      .filter((item): item is ProposalFeedback => item !== null),
    updatedAt: asString(record?.updated_at),
  };
}

/** One feedback item. A comment with no body is nothing to show. */
function readFeedback(value: unknown): ProposalFeedback | null {
  const record = asObject(value);
  const body = asString(record?.body);
  if (body === null) {
    return null;
  }
  return {
    author: asString(record?.author) ?? "",
    body,
    path: asString(record?.path),
    line: asNumber(record?.line),
    submittedAt: asString(record?.submitted_at),
    kind: asString(record?.kind) ?? "comment",
  };
}

/**
 * One conflict.
 *
 * Both halves or nothing: the id is what the detail and the resolve routes are
 * addressed by, and a path with no id is a conflict nothing on this side can
 * open. The count above it is read from the raw list, so a report that carries
 * bare paths still says how many there are.
 */
function readConflict(value: unknown): SyncConflict | null {
  const record = asObject(value);
  const id = asString(record?.id);
  const path = asString(record?.path);
  if (id === null || path === null) {
    return null;
  }
  return {
    id,
    path,
    kind: asString(record?.kind) ?? "",
    detectedAt: asString(record?.detected_at),
  };
}

/** Read a sync status out of the engine's own per-domain report. */
function readSyncStatus(payload: unknown): SyncStatus {
  const record = asObject(payload);
  const connected = asObject(record?.connection)?.connected;
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
    mode: asString(record?.mode),
    // Read tolerantly and off the aggregate's own block, which the route lifts
    // onto the per-domain report unchanged. Anything that is not a boolean -
    // an absent block, a block of nonsense, a string "true" - is "no answer"
    // rather than "not connected", because only `false` makes the card speak.
    connected: typeof connected === "boolean" ? connected : null,
    proposals: [
      ...asArray(record?.open_proposals),
      ...asArray(record?.declined_proposals),
    ]
      .map(readProposal)
      .filter((proposal): proposal is SyncProposal => proposal !== null),
    conflictList: asArray(record?.conflicts)
      .map(readConflict)
      .filter((conflict): conflict is SyncConflict => conflict !== null),
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

/** What a share would do, before anybody commits to doing it. */
export interface SharePlan {
  /**
   * `create`, `update`, `nothing_to_share`, `conflicts_pending` or
   * `proposal_diverged` - the server's own word for what the button would do,
   * which is also what decides whether there is a button at all.
   */
  action: string;
  /** The title the proposal would carry, the server's own if none was given. */
  effectiveTitle: string;
  changes: { path: string; kind: string }[];
  /** The proposal an `update` would go into; null for the other actions. */
  number: number | null;
  url: string | null;
  /**
   * How many conflicts are waiting, on a `conflicts_pending` plan; null on
   * every other action, and on a report that named none.
   *
   * The one number that turns "settle the conflicts first" into something a
   * reader can size the work from before opening the screen that settles them.
   */
  count: number | null;
}

/**
 * What sharing would do right now.
 *
 * A read that writes the working tree: the route pulls the origin first so the
 * plan is about the team's current state rather than about a stale copy, which
 * is why a read-only instance refuses it.
 */
export async function fetchShareChanges(domain: string): Promise<SharePlan> {
  const record = asObject(
    await api<unknown>(`/domains/${encodeSegment(domain)}/sync/changes`),
  );
  return {
    action: asString(record?.action) ?? "create",
    effectiveTitle: asString(record?.effective_title) ?? "",
    changes: asArray(record?.changes)
      .map((entry) => {
        const change = asObject(entry);
        const path = asString(change?.path);
        return path === null
          ? null
          : { path, kind: asString(change?.kind) ?? "" };
      })
      .filter(
        (change): change is { path: string; kind: string } => change !== null,
      ),
    number: asNumber(record?.number),
    url: asString(record?.url),
    count: asNumber(record?.count),
  };
}

/**
 * Share this domain's local changes as a proposal, or into the open one.
 *
 * The outcome comes back as the engine's own report rather than as a shape read
 * here: it is five different answers (`proposed`, `updated`,
 * `nothing_to_share`, `conflicts_pending`, `proposal_diverged`) and the screen
 * that asked is the one that knows which of them it is looking for.
 */
export async function shareDomain(
  domain: string,
  body: { title?: string; description?: string },
): Promise<unknown> {
  return api<unknown>(`/domains/${encodeSegment(domain)}/sync/share`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

/**
 * What a withdraw did, on the forge and on this copy.
 *
 * The three file lists are the reason this is read rather than passed through
 * as the engine's own JSON: a revert rewrites the working tree and re-indexes
 * the domain, so a caller has to be able to tell a withdraw that only closed a
 * pull request from one that moved files under every list on the screen.
 */
export interface WithdrawReceipt {
  number: number;
  /** Whether a live pull request was closed, as opposed to only a record. */
  closed: boolean;
  /** What the record now says, which is `withdrawn`. */
  status: string;
  /** Files a revert put back from the origin, and files it removed. */
  restored: string[];
  deleted: string[];
  /** Files a reviewer amended on the branch, which a revert leaves alone. */
  skippedDiverged: string[];
}

/** Close a proposal on the forge; `revert` also restores the shared files. */
export async function withdrawProposal(
  domain: string,
  number: number,
  revert: boolean,
): Promise<WithdrawReceipt> {
  const record = asObject(
    await api<unknown>(
      `/domains/${encodeSegment(domain)}/sync/proposals/${number}/withdraw`,
      { method: "POST", body: JSON.stringify({ revert }) },
    ),
  );
  return {
    // The number that was asked for, when the answer did not repeat it.
    number: asNumber(record?.number) ?? number,
    closed: record?.closed === true,
    status: asString(record?.status) ?? "withdrawn",
    restored: asStrings(record?.restored),
    deleted: asStrings(record?.deleted),
    skippedDiverged: asStrings(record?.skipped_diverged),
  };
}

/** One conflict with every side of it, for the screen that settles it. */
export interface ConflictDetail {
  id: string;
  path: string;
  kind: string;
  /** The shared start, the local side and the team's, each null when the
   * stored side is not UTF-8 - `note` says so when one is. */
  base: string | null;
  local: string | null;
  upstream: string | null;
  note: string | null;
}

/** Both sides of one conflict, by id. */
export async function fetchConflict(
  domain: string,
  id: string,
): Promise<ConflictDetail> {
  const record = asObject(
    await api<unknown>(
      `/domains/${encodeSegment(domain)}/sync/conflicts/${encodeSegment(id)}`,
    ),
  );
  return {
    // The id that was asked for, when the answer did not repeat it: this is
    // the handle the resolve below is addressed by, and losing it would leave
    // a detail nothing can be done to.
    id: asString(record?.id) ?? id,
    path: asString(record?.path) ?? "",
    kind: asString(record?.kind) ?? "",
    base: asString(record?.base),
    local: asString(record?.local),
    upstream: asString(record?.upstream),
    note: asString(record?.note),
  };
}

/** Settle one conflict by id: `mine`, `theirs`, or `merged` with content. */
export async function resolveConflict(
  domain: string,
  id: string,
  resolution: string,
  content?: string,
): Promise<unknown> {
  return api<unknown>(
    `/domains/${encodeSegment(domain)}/sync/conflicts/${encodeSegment(id)}/resolve`,
    { method: "POST", body: JSON.stringify({ resolution, content }) },
  );
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
  /**
   * Preview only: entries that would be created. Renamed off the wire's `new`,
   * which reads as the operator rather than as a count at every use site.
   */
  newEntries: number;
  /** Preview only: entries whose path or permalink is already taken. */
  collides: number;
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
    // A preview tallies these two and an import tallies the four below, so
    // each pair arrives at zero in the other's report. Read tolerantly all the
    // same: a counter a report does not carry is none of them, not a hole a
    // counter line would print as `undefined`.
    newEntries: asCount(report.new),
    collides: asCount(report.collides),
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
