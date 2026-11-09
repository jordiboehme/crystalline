/**
 * Refresh what a write changed, even when its first read is still running.
 *
 * `invalidateQueries` alone loses one case. Its refetch passes
 * `cancelRefetch: true`, but query-core (5.105.0, `Query.fetch`) only honours
 * that when the query already holds data. A query on its first load has none,
 * so the refetch joins the read that is already on the wire. That read left
 * before the write committed, it answers with the old snapshot, and its
 * success clears the invalidated flag. The screen then shows a list without
 * the person's own write until the window gets focus again.
 *
 * So the queries in exactly that state - matched, refetched by this call,
 * fetching, no data yet - are cancelled first. `cancelQueries` reverts such a
 * query to its idle pending state, and the invalidate after it starts a fresh
 * read. A query with data needs no help: the invalidate's own `cancelRefetch`
 * restarts its fetch. A query this call would not refetch (an inactive one
 * under `refetchType: "active"`, the default) is never cancelled, because a
 * `fetchQuery` caller may be waiting on it and nothing would start it again.
 *
 * One cost: a `fetchQuery` or `ensureQueryData` call that joined an active
 * first load is rejected with a `CancelledError` instead of getting the old
 * snapshot, as query-core already does to such callers when the query has
 * data. No caller in Fluid joins a first load that way today; one that does
 * should catch the cancel and read again.
 *
 * The returned promise is the invalidate's, so awaiting this still means the
 * refetch has settled.
 */

import type {
  InvalidateQueryFilters,
  QueryClient,
} from "@tanstack/react-query";

export function refreshAfterWrite(
  queryClient: QueryClient,
  filters: InvalidateQueryFilters,
): Promise<void> {
  const refetchType = filters.refetchType ?? filters.type ?? "active";
  if (refetchType !== "none") {
    const { predicate } = filters;
    // Not awaited: the cancel and its revert happen synchronously, so the
    // invalidate below sees an idle query and starts a new read, and the
    // invalidated flag is set in the same tick, as a bare invalidate sets it.
    void queryClient.cancelQueries({
      ...filters,
      type: refetchType,
      fetchStatus: "fetching",
      predicate: (query) =>
        query.state.data === undefined &&
        (predicate === undefined || predicate(query)),
    });
  }
  return queryClient.invalidateQueries(filters);
}
