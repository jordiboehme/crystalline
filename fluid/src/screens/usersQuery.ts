/**
 * The users table's search, filters, sort and page as they live in the
 * address: `q`, `role`, `state`, `kind`, `github`, `seen_after`,
 * `seen_before`, `sort`, `dir`, `page` and `size`.
 *
 * Reading is forgiving on purpose. An address is something people paste,
 * shorten and keep in bookmarks across releases, so a value this screen does
 * not know falls back to its default without a word, and the table shows what
 * it can. Writing leaves every default out, so the plain list is the plain
 * address.
 */

import type {
  KindFilter,
  Role,
  SortColumn,
  SortDir,
  StateFilter,
} from "../api/model";
import type { UsersPageRequest } from "../api/users";

/** The page sizes on offer. */
export const PAGE_SIZES = [25, 50, 100] as const;

/** The page size when the address names none. */
export const DEFAULT_PAGE_SIZE = 50;

const ROLES: readonly Role[] = ["none", "viewer", "editor", "admin"];
const STATES: readonly StateFilter[] = ["active", "disabled"];
const KINDS: readonly KindFilter[] = [
  "password",
  "oidc",
  "proxy",
  "token_only",
];
const SORTS: readonly SortColumn[] = [
  "name",
  "display",
  "role",
  "state",
  "kind",
  "last_seen",
];
const DIRS: readonly SortDir[] = ["asc", "desc"];

/** The list as it reads with nothing asked. */
export const DEFAULT_USERS_REQUEST: UsersPageRequest = {
  q: "",
  role: null,
  state: null,
  kind: null,
  github: null,
  seenAfter: null,
  seenBefore: null,
  sort: "name",
  dir: "asc",
  page: 1,
  size: DEFAULT_PAGE_SIZE,
};

function oneOf<T extends string>(
  allowed: readonly T[],
  value: string | null,
): T | null {
  return value !== null && (allowed as readonly string[]).includes(value)
    ? (value as T)
    : null;
}

/** A day as `YYYY-MM-DD` that names a real date, else `null`. */
function day(value: string | null): string | null {
  if (value === null || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
  const at = new Date(`${value}T00:00:00Z`);
  return !Number.isNaN(at.getTime()) && at.toISOString().slice(0, 10) === value
    ? value
    : null;
}

/** The request the address asks for, every unknown value at its default. */
export function readUsersSearch(params: URLSearchParams): UsersPageRequest {
  const github = params.get("github");
  const page = params.get("page");
  const size = Number(params.get("size"));
  return {
    q: params.get("q") ?? "",
    role: oneOf(ROLES, params.get("role")),
    state: oneOf(STATES, params.get("state")),
    kind: oneOf(KINDS, params.get("kind")),
    github: github === "true" ? true : github === "false" ? false : null,
    seenAfter: day(params.get("seen_after")),
    seenBefore: day(params.get("seen_before")),
    sort: oneOf(SORTS, params.get("sort")) ?? DEFAULT_USERS_REQUEST.sort,
    dir: oneOf(DIRS, params.get("dir")) ?? DEFAULT_USERS_REQUEST.dir,
    page: page !== null && /^[1-9]\d{0,5}$/.test(page) ? Number(page) : 1,
    size: (PAGE_SIZES as readonly number[]).includes(size)
      ? size
      : DEFAULT_PAGE_SIZE,
  };
}

/** The address for a request, with every default left out. */
export function writeUsersSearch(request: UsersPageRequest): URLSearchParams {
  const params = new URLSearchParams();
  if (request.q !== "") params.set("q", request.q);
  if (request.role) params.set("role", request.role);
  if (request.state) params.set("state", request.state);
  if (request.kind) params.set("kind", request.kind);
  if (request.github !== null) params.set("github", String(request.github));
  if (request.seenAfter) params.set("seen_after", request.seenAfter);
  if (request.seenBefore) params.set("seen_before", request.seenBefore);
  if (request.sort !== DEFAULT_USERS_REQUEST.sort) {
    params.set("sort", request.sort);
  }
  if (request.dir !== DEFAULT_USERS_REQUEST.dir) params.set("dir", request.dir);
  if (request.page !== 1) params.set("page", String(request.page));
  if (request.size !== DEFAULT_PAGE_SIZE) {
    params.set("size", String(request.size));
  }
  return params;
}

/**
 * A change to what is listed: a search, a filter, the sort or the page size.
 * Any of them starts again at the first page, since the old page number
 * points into a different list.
 */
export function narrowed(
  request: UsersPageRequest,
  change: Partial<Omit<UsersPageRequest, "page">>,
): UsersPageRequest {
  return { ...request, ...change, page: 1 };
}

/** Whether any filter or search is applied. */
export function isFiltered(request: UsersPageRequest): boolean {
  return (
    request.q.trim() !== "" ||
    request.role !== null ||
    request.state !== null ||
    request.kind !== null ||
    request.github !== null ||
    request.seenAfter !== null ||
    request.seenBefore !== null
  );
}
