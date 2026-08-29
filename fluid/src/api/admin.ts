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
import {
  asArray,
  asNumber,
  asNumbers,
  asObject,
  asString,
  asStrings,
} from "./json";
import type {
  ArchiveReport as ArchiveReportWire,
  CreateDomainWireBody,
  GithubIdentityResponse,
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

/**
 * One account's own GitHub identity, as the profile card renders and polls it.
 *
 * The instance connection's personal counterpart, and the same promise holds:
 * no token material, ever. Only whose identity it is, whether a credential is
 * on file, the login it authenticated as, since when and where it lives.
 */
export interface GithubIdentity {
  /** The account it belongs to, which is always the caller's own. */
  account: string;
  /** Whether a personal credential is on file for that account. */
  connected: boolean;
  /** The GitHub login it authenticated as, when connected. */
  login: string | null;
  /** When the credential was stored, RFC 3339; null when disconnected. */
  connectedAt: string | null;
  /**
   * `keyring` or `file`; null when disconnected. Never `environment`: the
   * environment supplies the machine's credential and never a personal one.
   */
  tokenStore: string | null;
  /** This account's device flow waiting for the browser side, or null. */
  pending: GithubPending | null;
  /**
   * This account's finished flow's failure, in the server's words. Reported on
   * exactly one read after the flow ends, then cleared.
   */
  error: string | null;
}

/**
 * The cache key of the caller's own GitHub identity, and the third key in this
 * app that sits outside the `["domains", ...]` family.
 *
 * A personal credential is not domain content: react-query invalidates by
 * prefix, and an identity filed under `["domains"]` would be re-read by every
 * bulk domain invalidation in the app - including the ones a share fires on its
 * way out, which is precisely when this card is being watched. It is not the
 * instance connection's key either: the two are different credentials with
 * different lifetimes, and a disconnect on one must not blank the other's card.
 */
export const MY_GITHUB_IDENTITY_KEY = ["me-github-identity"] as const;

/**
 * Read an identity payload, whatever arrived.
 *
 * Defensive rather than cast, for the reason {@link readGithubStatus} is: this
 * one is also the device flow's poll, read on a timer while a background flow
 * finishes, and a field that is briefly missing should leave the card saying
 * "not connected" rather than throwing inside a render.
 */
export function readMyGithubIdentity(payload: unknown): GithubIdentity {
  const record = asObject(payload);
  return {
    account: asString(record?.account) ?? "",
    connected: record?.connected === true,
    login: asString(record?.login),
    connectedAt: asString(record?.connected_at),
    tokenStore: asString(record?.token_store),
    pending: readPending(record?.pending),
    error: asString(record?.error),
  };
}

/**
 * The caller's own identity as it stands. Also the device flow's poll: the
 * flow finishes in another window, so there is no event to wait for.
 *
 * No account in the path, because the session already names one: this surface
 * manages the caller's own credential and no one else's.
 */
export async function fetchMyGithubIdentity(): Promise<GithubIdentity> {
  return readMyGithubIdentity(
    await api<GithubIdentityResponse>("/me/github-identity"),
  );
}

/**
 * Start a device-code sign-in for the caller's own identity.
 *
 * A second call from the same account reports the code already outstanding, so
 * a double press is safe; one made while ANOTHER identity's sign-in is in
 * flight is refused 409, which is the server's sentence to show as it stands.
 */
export async function startMyGithubIdentityDevice(): Promise<GithubIdentity> {
  return readMyGithubIdentity(
    await api<GithubIdentityResponse>("/me/github-identity/connect", {
      method: "POST",
    }),
  );
}

/**
 * Connect the caller's own identity with a personal access token.
 *
 * `PUT`, unlike the instance surface's `POST`: this replaces the caller's one
 * identity rather than adding to a collection, so re-pasting a token is the
 * same request twice with the same result. The token goes out in this one body
 * and is held nowhere - the answer is the same token-material-free identity
 * every other verb here returns.
 */
export async function connectMyGithubIdentityToken(
  token: string,
): Promise<GithubIdentity> {
  return readMyGithubIdentity(
    await api<GithubIdentityResponse>("/me/github-identity/token", {
      method: "PUT",
      body: JSON.stringify({ token }),
    }),
  );
}

/**
 * Forget the caller's own credential. Idempotent, and the instance connection
 * is untouched: each credential lives in its own store entry.
 */
export async function disconnectMyGithubIdentity(): Promise<GithubIdentity> {
  return readMyGithubIdentity(
    await api<GithubIdentityResponse>("/me/github-identity", {
      method: "DELETE",
    }),
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
  /**
   * The GitHub login the share that wrote this acted as, or null when nobody
   * is named.
   *
   * Null twice over: a proposal shared before the engine recorded this carries
   * nothing, and so does one shared by a credential with no login to name. A
   * row says who owns a layer where there is an answer and says nothing where
   * there is not - a chain's layers can belong to different people, and a
   * blank is never worth a guess.
   */
  authorLogin: string | null;
}

/**
 * The machine OWNER's personal slot, as the status report names it in personal
 * mode.
 *
 * Never the browser's own acting identity: a share made from Fluid goes out as
 * the SESSION's identity, which `/me/github-identity` answers for. This is what
 * a CLI or a local stdio-MCP share would resolve, reported here so a client can
 * say so where that is what it is describing.
 */
export interface OwnerIdentity {
  /** The account the slot belongs to, the fixed local owner name. */
  account: string;
  /** Whether a personal credential is on file for it. */
  connected: boolean;
  /** The login it authenticated as, when there is one. */
  user: string | null;
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
  /**
   * The chain's own number on the forge, or null when nothing is stacked.
   *
   * Never the gate on whether there is a chain to draw: on the stacked path
   * this is null for as long as the call that groups the layers on the forge
   * has not landed, and a screen that keyed off it would print "stack #null"
   * over a chain that is perfectly real. {@link SyncStatus.stackLinkPending}
   * is what that state says out loud.
   */
  stackNumber: number | null;
  /**
   * The declined layers still carrying open layers above them, by number.
   *
   * Empty when the chain is sound, and the one stack fact a screen must not
   * hide: a wedged chain cannot grow until one of these is withdrawn or the
   * chain is repaired, and the number is what either verb is addressed by.
   */
  stackWedged: number[];
  /** A rebuild left half-done, which the next share or withdraw finishes. */
  repairPending: boolean;
  /** Every layer exists, but they are not grouped on the forge yet. */
  stackLinkPending: boolean;
  /**
   * Whose credential a write to this origin goes out on: `instance` for the
   * one machine credential, `personal` for the acting person's own. Null when
   * the report did not say.
   *
   * The mode only, never the credential itself. In the browser the acting
   * person is the SESSION, so this says which QUESTION to ask and
   * `/me/github-identity` answers it: a dialog in personal mode asks whether
   * this session has an identity, and one in instance mode asks nothing at
   * all. Anything that is not the word `personal` leaves a dialog exactly as
   * it was, which is what keeps an older report drawing the dialog it always
   * drew.
   */
  shareIdentity: string | null;
  /**
   * The machine owner's personal slot, sent in personal mode only, or null.
   *
   * Read because the report carries it, and pointedly not what a browser
   * dialog acts on: see {@link OwnerIdentity}.
   */
  ownerIdentity: OwnerIdentity | null;
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
 * One team domain's standing, as the instance-wide summary counts it.
 *
 * Counts and a name, and nothing else. The entry carries its repository, its
 * branch and when it was last checked as well, and none of them is read here on
 * purpose: this is what a share action needs to decide whether to offer itself
 * and what to fill a picker with, while the card that draws a repository reads
 * the per-domain report {@link SyncStatus} is made of.
 */
export interface SyncSummaryEntry {
  domain: string;
  /** Unshared local work, as a count. */
  localChanges: number;
  openProposals: number;
  declinedProposals: number;
  conflicts: number;
  /**
   * The declined layers still carrying open layers above them, by number.
   *
   * The one stack fact a picker must not hide, which is why it rides along
   * with a row that otherwise carries only counts: a wedged chain cannot
   * grow, so a domain offered without it is a domain somebody picks and then
   * finds out about from a refusal. Where the domain sits IN its chain is
   * detail rather than a decision, and stays on the per-domain report.
   */
  stackWedged: number[];
  /** A rebuild left half-done, which the next share or withdraw finishes. */
  repairPending: boolean;
  /** Every layer exists, but they are not grouped on the forge yet. */
  stackLinkPending: boolean;
}

/** Where every team domain on this instance stands, in one read. */
export interface SyncSummary {
  /**
   * Whether this instance has a GitHub credential on file, or null when the
   * report carried no connection block at all. The three states are three
   * different sentences, for the reason {@link SyncStatus.connected} spells
   * out.
   */
  connected: boolean | null;
  /** One entry per team domain; empty on an instance that has none. */
  domains: SyncSummaryEntry[];
}

/**
 * The cache key of the instance-wide sync summary, and the second key in this
 * app that deliberately sits outside the `["domains", ...]` family.
 *
 * Reading the summary probes GitHub for every team domain at once, so it is not
 * a cache of domain content: react-query invalidates by prefix, and a summary
 * filed under `["domains"]` would be refetched - which is to say, would probe -
 * by every bulk domain invalidation in the app, including the ones a share and
 * an import fire on their way out. {@link sharePlanKey} carries the same
 * reasoning for the same reason.
 */
export const SYNC_SUMMARY_KEY = ["sync-summary"] as const;

/**
 * How long a summary stays fresh, in milliseconds.
 *
 * Here rather than at one use site because there are two - the frame's share
 * action and the picker it opens - and a picker that considered the frame's
 * answer stale would probe every origin again in the act of being opened,
 * which is a round trip to GitHub per domain to draw a list already in hand.
 */
export const SYNC_SUMMARY_STALE_MS = 30_000;

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
    // Absent on everything shared before the engine recorded it, and null
    // wherever the acting credential had no login: both read as "nobody
    // named", and a row draws nothing rather than a gap.
    authorLogin: asString(record?.author_login),
  };
}

/**
 * The owner's personal slot, or null when the report carries none.
 *
 * The account is the gate: instance mode sends no block at all, and a block
 * that names no account is nothing a screen could say a sentence about.
 */
function readOwnerIdentity(value: unknown): OwnerIdentity | null {
  const record = asObject(value);
  const account = asString(record?.account);
  if (account === null) {
    return null;
  }
  return {
    account,
    connected: record?.connected === true,
    user: asString(record?.user),
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
  const connection = asObject(record?.connection);
  const connected = connection?.connected;
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
    // The four chain keys, which the route always sends and an older report
    // never did: quiet defaults rather than holes, so one reader handles the
    // stacked path and the unstacked one.
    stackNumber: asNumber(record?.stack_number),
    stackWedged: asNumbers(record?.stack_wedged),
    repairPending: record?.repair_pending === true,
    stackLinkPending: record?.stack_link_pending === true,
    // Both off the connection block, where the route puts them, and both
    // tolerant: a mode that is not a word and a slot that is not a record are
    // "this report does not say", which every reader treats as the default
    // mode rather than as personal.
    shareIdentity: asString(connection?.share_identity),
    ownerIdentity: readOwnerIdentity(connection?.owner_identity),
  };
}

/** Where this team domain stands. 404 for a domain with no origin. */
export async function fetchSyncStatus(domain: string): Promise<SyncStatus> {
  return readSyncStatus(
    await api<unknown>(`/domains/${encodeSegment(domain)}/sync`),
  );
}

/**
 * One summary entry.
 *
 * The name or nothing: it is the handle a share is addressed by, so an entry
 * without one is dropped rather than offered as a row that would open a dialog
 * pointing at no domain.
 */
function readSummaryEntry(value: unknown): SyncSummaryEntry | null {
  const record = asObject(value);
  const domain = asString(record?.domain);
  if (domain === null) {
    return null;
  }
  return {
    domain,
    localChanges: asCount(record?.local_changes),
    openProposals: asCount(record?.open_proposals),
    declinedProposals: asCount(record?.declined_proposals),
    conflicts: asCount(record?.conflicts),
    stackWedged: asNumbers(record?.stack_wedged),
    repairPending: record?.repair_pending === true,
    stackLinkPending: record?.stack_link_pending === true,
  };
}

/**
 * Where every team domain stands, in counts. Admin only.
 *
 * An instance with GitHub switched off refuses this with a 409, and an instance
 * with no credential on file is reported rather than refused - `connected` is
 * false and the entries are local state alone.
 */
export async function fetchSyncSummary(): Promise<SyncSummary> {
  const record = asObject(await api<unknown>("/sync"));
  const connected = asObject(record?.connection)?.connected;
  return {
    // Only a literal boolean is an answer, the way the per-domain report reads
    // the same block: anything else is "this report does not say".
    connected: typeof connected === "boolean" ? connected : null,
    domains: asArray(record?.domains)
      .map(readSummaryEntry)
      .filter((entry): entry is SyncSummaryEntry => entry !== null),
  };
}

/** Pull this team domain's origin now. */
export async function syncDomain(domain: string): Promise<unknown> {
  return api<unknown>(`/domains/${encodeSegment(domain)}/sync`, {
    method: "POST",
  });
}

/**
 * One file a share would carry, as the plan reports it.
 *
 * `lastAuthor` is the OKF actor the file's own frontmatter records as having
 * written it - `human:ada` for a person, an agent's own name for an agent -
 * and null wherever there is nothing to read: a deleted file, a file edited
 * outside the engine, an older server that names nobody. It is last-writer
 * provenance rather than authorship, and it is what lets the share dialog
 * open with a person's own work already ticked.
 */
export interface ShareChange {
  path: string;
  kind: string;
  lastAuthor: string | null;
}

/** What a share would do, before anybody commits to doing it. */
export interface SharePlan {
  /**
   * `create`, `update`, `stack`, `amend`, `nothing_to_share`,
   * `conflicts_pending` or `proposal_diverged` - the server's own word for
   * what the button would do, which is also what decides whether there is a
   * button at all.
   */
  action: string;
  /** The title the proposal would carry, the server's own if none was given. */
  effectiveTitle: string;
  changes: ShareChange[];
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
  /**
   * The open layer a `stack` would sit on, and its title; null on every other
   * action. The whole difference between stacking and opening a lone proposal
   * is what it lands on, so the plan names it.
   */
  topNumber: number | null;
  topTitle: string | null;
  /**
   * How many layers an `amend` would rebuild; null on every other action.
   *
   * The difference between amending the top layer and amending one under it:
   * the second re-bases work that is already in front of reviewers.
   */
  layersAbove: number | null;
}

/**
 * Where a shared proposal sits in its chain, as the share outcome reports it.
 *
 * Two fields with one rule between them, which is why they are read together
 * rather than field by field at a use site: `stackPosition` is `[layer, open
 * layers]` with a 1-based layer and is the gate on whether there is a chain at
 * all, while `stackNumber` is named only when there is one. On the stacked
 * path the position is always set and the number is null until the call that
 * groups the chain on the forge lands, so a renderer that keyed off the number
 * would print "stack #null" over a chain that is perfectly real.
 */
export interface StackPlacement {
  stackNumber: number | null;
  /** `[layer, open layers]`, 1-based, or null off the stacked path. */
  stackPosition: [number, number] | null;
}

/** Read a placement off whatever the share outcome carried. */
export function readStackPlacement(payload: unknown): StackPlacement {
  const record = asObject(payload);
  const position = asArray(record?.stack_position);
  const layer = asNumber(position[0]);
  const open = asNumber(position[1]);
  return {
    stackNumber: asNumber(record?.stack_number),
    // Both halves or nothing: half a position says neither which layer this
    // is nor how many there are, and either alone is unprintable.
    stackPosition: layer === null || open === null ? null : [layer, open],
  };
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
          : {
              path,
              kind: asString(change?.kind) ?? "",
              // A server that names nobody reads exactly like a file nobody
              // is named for, which is the same thing to every reader of it.
              lastAuthor: asString(change?.last_author),
            };
      })
      .filter((change): change is ShareChange => change !== null),
    number: asNumber(record?.number),
    url: asString(record?.url),
    count: asNumber(record?.count),
    topNumber: asNumber(record?.top_number),
    topTitle: asString(record?.top_title),
    layersAbove: asNumber(record?.layers_above),
  };
}

/**
 * Share this domain's local changes as a proposal, as a new layer on the chain
 * already open, or into an open layer named by number.
 *
 * The outcome comes back as the engine's own report rather than as a shape read
 * here: it is five different answers (`proposed`, `updated`,
 * `nothing_to_share`, `conflicts_pending`, `proposal_diverged`) and the screen
 * that asked is the one that knows which of them it is looking for.
 * {@link readStackPlacement} is how the two that landed say where in the chain
 * they landed.
 */
export async function shareDomain(
  domain: string,
  body: {
    title?: string;
    description?: string;
    proposal?: number;
    /**
     * The files to carry, when they are some of them rather than all of them.
     * Left out entirely for a share of everything, so the common case is the
     * request it always was.
     */
    files?: string[];
  },
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
  /**
   * Files whose pre-share content is nowhere to be had, so no revert could put
   * them back. A different reason from {@link WithdrawReceipt.skippedDiverged}
   * and worth its own sentence: nobody moved on from these, they simply cannot
   * be restored, and somebody has to know which ones.
   */
  skippedReverts: string[];
  /** Whether the chain around the withdrawn layer was rebuilt. */
  repaired: boolean;
  /**
   * The NEW stack number that rebuild allocated, or null.
   *
   * Null covers both "no repair happened" and "the survivors no longer make a
   * chain", so it is read together with {@link WithdrawReceipt.repaired}
   * rather than alone. Stack numbers come off the same sequence as proposal
   * numbers, so the old one never comes back.
   */
  restacked: number | null;
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
    skippedReverts: asStrings(record?.skipped_reverts),
    repaired: record?.repaired === true,
    restacked: asNumber(record?.restacked),
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
