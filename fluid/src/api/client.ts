/**
 * The one way this app talks to Crystalline.
 *
 * The OpenAPI snapshot the types are generated from carries no security scheme
 * and no CSRF parameter by design: the session is a cookie the browser attaches
 * on its own, and the token that guards it is a header only a same-origin
 * client can know. So the types are generated and this wrapper is written by
 * hand, and everything the transport needs lives here rather than at each call
 * site.
 */

/** Where the API is mounted. Same origin as the app, always. */
export const API_BASE = "/api/v1";

/** The header a mutating request echoes the session's CSRF token in. */
export const CSRF_HEADER = "X-CSRF-Token";

/** The methods that carry no side effect, so the server exempts them from CSRF. */
const SAFE_METHODS = new Set(["GET", "HEAD", "OPTIONS", "TRACE"]);

/**
 * A failure from the API, parsed from the RFC 9457 problem detail every route
 * answers with (`application/problem+json`, and no other failure shape exists
 * on this surface: extractor rejections, the 404 and the 405 fallback all take
 * this form).
 *
 * `status` is the only thing worth branching on. 400 and 422 both mean the
 * request was rejected and `detail` says why, so neither deserves special
 * casing, and 403 can arrive from any route including `GET /auth/me` when an
 * account behind an SSO proxy has been disabled. Nothing here retries: a
 * refused request is refused, and a client that retried a 403 probe would spin
 * forever.
 */
export class ApiProblem extends Error {
  /** The HTTP status. `0` when the request never reached the server. */
  readonly status: number;
  /** The short, stable summary of the problem type. */
  readonly title: string;
  /** The specific occurrence, safe to show to the person using the app. */
  readonly detail: string;

  constructor(status: number, title: string, detail: string) {
    super(detail || title);
    this.name = "ApiProblem";
    this.status = status;
    this.title = title;
    this.detail = detail;
  }
}

/**
 * What to show a reader about a failure: the server's own words where there
 * are any, and the error's message where there are not.
 *
 * Every failure a screen can hold is either an {@link ApiProblem} the server
 * described or an `Error` the app itself threw, and the rule for both is the
 * same everywhere: print what it says. This is that rule in one place, so a
 * surface cannot quietly opt out of it by writing its own sentence instead.
 */
export function problemDetail(error: Error): string {
  return error instanceof ApiProblem ? error.detail : error.message;
}

/**
 * The CSRF token of the current session, held in memory only.
 *
 * The session cookie is `HttpOnly`, so a reload cannot read the token back out
 * of it. Both `POST /auth/login` and `GET /auth/me` hand one out, and the probe
 * is what the app opens on, so a reloaded tab gets its token from there.
 * `GET /auth/me` answers `null` for the anonymous viewer and for a
 * trusted-header identity, neither of which has a session to protect: those
 * requests are guarded by the shape they are allowed to have instead, which is
 * why every request with a body here goes out as `application/json` and never
 * as a form content type.
 */
let csrfToken: string | null = null;

/** Remember the token from a login or a `GET /auth/me`, or forget it on logout. */
export function setCsrfToken(token: string | null): void {
  csrfToken = token;
}

/** The token currently held, if any. */
export function getCsrfToken(): string | null {
  return csrfToken;
}

/**
 * Encode one path segment. Slashes are escaped, because a segment that is not
 * a permalink must never widen the path it sits in.
 */
export function encodeSegment(value: string): string {
  return encodeURIComponent(value);
}

/**
 * Encode a permalink, which is itself a path: each of its segments is encoded
 * on its own and the separators stay literal slashes, because that is how the
 * route matches it (`/domains/{domain}/engrams/{*permalink}`).
 *
 * Encoding the whole permalink in one go would turn `notes/deep/gamma` into
 * `notes%2Fdeep%2Fgamma`, which is a different, missing engram: the silent 404
 * this function exists to prevent.
 */
export function encodePermalink(value: string): string {
  return value.split("/").map(encodeURIComponent).join("/");
}

/** The path of one engram, with the permalink's own slashes preserved. */
export function engramPath(domain: string, permalink: string): string {
  return `/domains/${encodeSegment(domain)}/engrams/${encodePermalink(permalink)}`;
}

/**
 * Call the API. `path` is relative to {@link API_BASE} and already encoded (see
 * {@link encodeSegment} and {@link engramPath}).
 *
 * Resolves with the parsed JSON body, or `undefined` when the response carries
 * none. Rejects with an {@link ApiProblem} for every failure: one the server
 * described in problem+json, one the network never delivered, and one wearing a
 * success status while carrying a body this client cannot read.
 */
export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const method = (init.method ?? "GET").toUpperCase();
  const headers = new Headers(init.headers);
  if (!SAFE_METHODS.has(method)) {
    if (!headers.has("Content-Type")) {
      headers.set("Content-Type", "application/json");
    }
    if (csrfToken !== null) {
      headers.set(CSRF_HEADER, csrfToken);
    }
  }

  let response: Response;
  try {
    response = await fetch(`${API_BASE}${path}`, {
      ...init,
      method,
      headers,
      // The session is a cookie, and the API is same origin in development too
      // (Vite proxies /api). No CORS layer exists on that surface and none may
      // be added, so "same-origin" is both what is needed and all that works.
      credentials: "same-origin",
    });
  } catch {
    throw new ApiProblem(
      0,
      "network error",
      "could not reach the server: it may be down, or this browser may be offline",
    );
  }

  if (!response.ok) {
    throw await problemFrom(response);
  }
  const body = await readBody(response);
  if (body.kind === "foreign" || body.kind === "unreadable") {
    throw unusableBodyProblem(response, body);
  }
  return (body.kind === "json" ? body.value : undefined) as T;
}

/**
 * Turn a failed response into an {@link ApiProblem}.
 *
 * The transport status wins over the one mirrored in the body: they agree by
 * contract, and when they cannot (a proxy answered, so there is no body at all)
 * the transport is the one telling the truth. A failure whose body is not the
 * problem detail it should be still becomes an `ApiProblem` on its status
 * alone: the request failed either way, and that is the fact the caller needs.
 */
async function problemFrom(response: Response): Promise<ApiProblem> {
  const fallbackTitle = response.statusText || "request failed";
  const body = await readBody(response);
  if (
    body.kind === "json" &&
    body.value !== null &&
    typeof body.value === "object"
  ) {
    const problem = body.value as Partial<Record<"title" | "detail", unknown>>;
    const title =
      typeof problem.title === "string" && problem.title !== ""
        ? problem.title
        : fallbackTitle;
    const detail =
      typeof problem.detail === "string" && problem.detail !== ""
        ? problem.detail
        : title;
    return new ApiProblem(response.status, title, detail);
  }
  return new ApiProblem(response.status, fallbackTitle, fallbackTitle);
}

/**
 * What a response body turned out to be. Four outcomes rather than two,
 * because "there was nothing to read", "there was something and it was not
 * JSON" and "the reading itself failed" are different facts and the caller
 * answers them differently.
 */
type Body =
  /** A 204, or a response that carried no bytes. */
  | { kind: "empty" }
  /** A JSON body, parsed. */
  | { kind: "json"; value: unknown }
  /** Bytes this client cannot use, with the content type that announced them. */
  | { kind: "foreign"; contentType: string; announcedJson: boolean }
  /** The headers arrived and the stream broke before the bytes did. */
  | { kind: "unreadable" };

/** Read the body once and classify it. Never throws. */
async function readBody(response: Response): Promise<Body> {
  if (response.status === 204) {
    return { kind: "empty" };
  }
  const contentType = response.headers.get("content-type") ?? "";
  // A read that fails is its own outcome, never an empty body. `fetch`
  // resolves as soon as the headers land, so a connection dropped mid-stream
  // rejects here on a 200, and calling that "empty" would hand the caller
  // `undefined` for a listing - which the screens above render as "no domains
  // are registered on this instance yet". That is a claim about the knowledge
  // base, made because a read failed.
  let text: string;
  try {
    text = await response.text();
  } catch {
    return { kind: "unreadable" };
  }
  if (text === "") {
    return { kind: "empty" };
  }
  if (!contentType.includes("json")) {
    return { kind: "foreign", contentType, announcedJson: false };
  }
  try {
    return { kind: "json", value: JSON.parse(text) as unknown };
  } catch {
    return { kind: "foreign", contentType, announcedJson: true };
  }
}

/**
 * The failure for a 2xx response whose body this client cannot use.
 *
 * A captive portal, a misconfigured proxy or an HTML sign-in page answered
 * with 200 all look like success to `fetch`, and so does a response whose
 * stream broke after the headers. Handing the caller `undefined` for one would
 * push the failure into whatever touches the missing field next, as a
 * `TypeError` naming nothing useful, or worse: on a list route `undefined`
 * reads as an empty list, and the screen says the instance holds nothing. It
 * is a failed request, so it leaves here as one.
 */
function unusableBodyProblem(
  response: Response,
  body: Extract<Body, { kind: "foreign" | "unreadable" }>,
): ApiProblem {
  return new ApiProblem(
    response.status,
    "unexpected response",
    body.kind === "unreadable"
      ? "the response body could not be read: the connection may have dropped before the answer was complete"
      : foreignBodyDetail(body),
  );
}

/** Why a body this client cannot parse is not the answer it was expecting. */
function foreignBodyDetail(body: Extract<Body, { kind: "foreign" }>): string {
  const named =
    body.contentType === ""
      ? "no content type"
      : `content type "${body.contentType}"`;
  return body.announcedJson
    ? `the server answered ${named} but the body is not valid JSON`
    : `expected a JSON body, but the server answered ${named}: something other than Crystalline may have answered this request`;
}
