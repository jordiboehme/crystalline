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
 * none. Rejects with an {@link ApiProblem} for every failure, including one the
 * network never delivered.
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
  return (await readBody(response)) as T;
}

/**
 * Turn a failed response into an {@link ApiProblem}.
 *
 * The transport status wins over the one mirrored in the body: they agree by
 * contract, and when they cannot (a proxy answered, so there is no body at all)
 * the transport is the one telling the truth.
 */
async function problemFrom(response: Response): Promise<ApiProblem> {
  const fallbackTitle = response.statusText || "request failed";
  const body = await readBody(response).catch(() => undefined);
  if (body !== undefined && body !== null && typeof body === "object") {
    const problem = body as Partial<Record<"title" | "detail", unknown>>;
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
 * The response body as JSON, or `undefined` when there is none to read: a 204,
 * or anything that did not announce itself as JSON (a proxy's HTML error page,
 * say, which tells a client nothing it can use).
 */
async function readBody(response: Response): Promise<unknown> {
  if (response.status === 204) {
    return undefined;
  }
  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.includes("json")) {
    return undefined;
  }
  const text = await response.text();
  if (text === "") {
    return undefined;
  }
  return JSON.parse(text) as unknown;
}
