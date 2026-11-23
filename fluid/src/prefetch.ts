/**
 * Warming a lazy screen's chunk while the pointer is still on its way to it.
 *
 * Every screen behind a `lazy` route in `routes.tsx` costs one extra request
 * the first time somebody opens it, and that request starts when the click
 * lands - which is the worst possible moment, because it is the moment a
 * reader starts waiting. A pointer entering a link, or the keyboard landing
 * on it, says the same thing a click says a few hundred milliseconds early,
 * and that is the whole trick: the fetch starts on the intent rather than on
 * the act, and by the time the route changes the chunk is usually already in
 * the module cache.
 *
 * Nothing here is a promise anybody waits on. A warm that has not finished
 * when the click lands is not a failure - the route's own `Suspense` fallback
 * covers it exactly as it would have without any of this - and a warm that
 * never happens because nobody hovered anything costs nothing at all. That is
 * why this is spread onto links rather than run on a timer at startup: a
 * reader who never opens an engram never fetches the engram screen.
 *
 * The imports are the same specifiers `routes.tsx` uses, which is what makes
 * them the same chunk rather than a second copy: the bundler keys a dynamic
 * import by the module it resolves to, and the module cache makes every call
 * after the first one free.
 */

/**
 * What has been asked for already, by name. A name leaves the set when its
 * import fails, so the next hover tries again; a name whose import worked
 * stays, and asking twice never even calls `import()` twice.
 */
const warmed = new Set<string>();

/**
 * Start loading one chunk, once. Idempotent, and silent on failure: a warm
 * that did not arrive is not an error, the route's own boundary handles the
 * click that follows.
 */
export function warm(name: string, load: () => Promise<unknown>): void {
  if (warmed.has(name)) {
    return;
  }
  warmed.add(name);
  load().catch(() => {
    warmed.delete(name);
  });
}

/** Warm the reading screen: the sidebar, the lists and the graph lead here. */
export function prefetchEngramPage(): void {
  warm("engram-page", () => import("./screens/EngramPage"));
}

/** Warm the engram editor, which is where the reading screen's Edit goes. */
export function prefetchEngramEditor(): void {
  warm("engram-editor", () => import("./screens/EngramEditor"));
}

/** And the MANIFEST editor, from the domain page's Manifest section. */
export function prefetchManifestEditor(): void {
  warm("manifest-editor", () => import("./screens/ManifestEditor"));
}

/** The rest of the lazy screens, by what a link to them is called. */
const SCREENS = {
  search: () => import("./screens/Search"),
  domain: () => import("./screens/DomainHome"),
  domainSettings: () => import("./screens/DomainSettings"),
  graph: () => import("./screens/GraphView"),
  maintenance: () => import("./screens/Maintenance"),
  // The two admin pages open inside the Administration area's frame, so a
  // link to either warms the frame's chunk as well as its own.
  admin: () => import("./screens/AdminArea"),
  users: () =>
    Promise.all([
      import("./screens/AdminArea"),
      import("./screens/UsersAdmin"),
    ]),
  github: () =>
    Promise.all([
      import("./screens/AdminArea"),
      import("./screens/GithubSettings"),
    ]),
  profile: () => import("./screens/Profile"),
  reflection: () => import("./screens/ReflectionHome"),
} as const;

export type PrefetchScreen = keyof typeof SCREENS;

/**
 * The handlers a link to a lazy screen spreads onto itself, by screen: the
 * pointer arriving or the keyboard landing starts the chunk.
 */
export const PREFETCH = Object.fromEntries(
  (Object.keys(SCREENS) as PrefetchScreen[]).map((name) => {
    const start = () => {
      warm(name, SCREENS[name]);
    };
    return [name, { onPointerEnter: start, onFocus: start }];
  }),
) as Record<
  PrefetchScreen,
  { onPointerEnter: () => void; onFocus: () => void }
>;

/**
 * The two handlers a link that leads to an engram spreads onto itself.
 *
 * Both, rather than the pointer alone: a keyboard arrives at a link by focus
 * and has exactly the same head start to offer. Spread as one object so a
 * link site adds a line rather than a pair of lines it could get half right,
 * and frozen into a constant so every such link is warming on the same two
 * events.
 */
export const ENGRAM_PREFETCH = {
  onPointerEnter: prefetchEngramPage,
  onFocus: prefetchEngramPage,
} as const;
