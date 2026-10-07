/**
 * Where this app is served: the root of its host, or a path under it such as
 * `https://example.com/crystalline`.
 *
 * The server writes the path into `<base href>` when it serves `index.html`
 * (`crates/service/src/ui.rs`, and `sub_filter` in the nginx image), so the
 * base is read once from `document.baseURI`. Everything that leaves the
 * router goes through this module: the router's basename, every fetch, the
 * WebSocket and the event stream, full-page navigations, new windows and
 * the links people copy. `scripts/basePathRule.ts` fails the lint on any
 * other file that builds such an address itself.
 */

/** The base path of `doc`: `""` at the root, `"/crystalline"` under a path. */
export function basePathOf(
  doc: Pick<Document, "baseURI" | "querySelector">,
): string {
  // Without a base tag `baseURI` is the page address itself, and a deep
  // link would read as a prefix.
  if (doc.querySelector("base[href]") === null) {
    return "";
  }
  const path = new URL(doc.baseURI).pathname;
  return path.endsWith("/") ? path.slice(0, -1) : path;
}

/** `path` (starting with `/`) under `base`. */
export function joinBase(base: string, path: string): string {
  return `${base}${path}`;
}

/** `pathname` without `base`: `/` for the base itself, unchanged outside it. */
export function stripBaseFrom(base: string, pathname: string): string {
  if (base === "") return pathname;
  if (pathname === base) return "/";
  return pathname.startsWith(`${base}/`)
    ? pathname.slice(base.length)
    : pathname;
}

/** This page's base path, read once. */
export const BASE_PATH = basePathOf(document);

/** What the router is given: React Router's own default at the root. */
export const ROUTER_BASENAME = BASE_PATH === "" ? "/" : BASE_PATH;

/** Where the API is mounted. Same origin as the app, always. */
export const API_BASE = joinBase(BASE_PATH, "/api/v1");

/** An app path under this page's base. */
export function withBase(path: string): string {
  return joinBase(BASE_PATH, path);
}

/** A location's pathname without this page's base. */
export function stripBase(pathname: string): string {
  return stripBaseFrom(BASE_PATH, pathname);
}

/** The absolute url of an app path, for a link somebody copies. */
export function absoluteUrl(path: string): string {
  return `${window.location.origin}${withBase(path)}`;
}

/**
 * Navigate the whole page to `location`, never a fetch.
 *
 * A handful of answers on this API are places to go rather than data to
 * render - an OAuth decision's redirect back to the client that asked, a
 * single sign-on hop to the provider - and a background fetch would follow
 * either invisibly. This is the one seam every such screen calls, so a test
 * can watch where a screen decided to send the browser.
 */
export function navigateTo(location: string): void {
  window.location.assign(location);
}

/** Open `url` (absolute, or already under the base) in a new tab. */
export function openWindow(url: string, features: string): void {
  window.open(url, "_blank", features);
}

/** Open an app path in a new tab, under this page's base. */
export function openRoute(path: string): void {
  openWindow(withBase(path), "noopener");
}

/** A scheme (`https:`, `mailto:`) or a scheme-relative `//host`. */
const ABSOLUTE = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;

/**
 * A link target written inside a document, as an href that goes where it
 * went before the base tag existed: a fragment or a relative path resolves
 * against `here` (the page's own address), an app path that starts with `/`
 * gets the base in front, and an absolute url is left alone. Without this the
 * base tag would send `#heading` to the home page, at the root too.
 */
export function documentHrefFrom(
  base: string,
  href: string,
  here: string,
): string {
  if (ABSOLUTE.test(href)) return href;
  if (href.startsWith("/")) return joinBase(base, href);
  const url = new URL(href, here);
  return `${url.pathname}${url.search}${url.hash}`;
}

/** [`documentHrefFrom`] for this page. `undefined` stays `undefined`. */
export function documentHref(href: string | undefined): string | undefined {
  return href === undefined
    ? undefined
    : documentHrefFrom(BASE_PATH, href, window.location.href);
}
