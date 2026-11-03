import {
  QueryClient,
  QueryClientContext,
  useQuery,
} from "@tanstack/react-query";
import { useContext, useMemo } from "react";

import { changedVersions } from "../api/assetVersions";
import type { AssetHome } from "../api/files";
import { assetRefsIn, engramAssetHome } from "../editor/imageFormat";

/**
 * What a document drawn outside the app's provider (a bare markdown render)
 * reads through: it never fetches, because nothing there enables a query.
 */
const STANDALONE = new QueryClient();

/**
 * The engram file store of `domain` for a page that draws `source`, with
 * each file's address naming its current bytes once they changed since this
 * document first listed them (`changedVersions`). A file replaced or
 * rewritten by somebody else - a person on another tab, an agent - is
 * announced by a `files` event, which refetches the listing this reads, so
 * the picture is drawn anew without a reload.
 *
 * The listing is the cache entry the attachments section reads, so this adds
 * no request. A page that links no file asks nothing (`source` undefined
 * counts as none).
 */
export function useEngramAssetHome(
  domain: string | undefined,
  source: string | undefined,
): AssetHome | undefined {
  const queryClient = useContext(QueryClientContext) ?? STANDALONE;
  const plain = useMemo(
    () => (domain === undefined ? undefined : engramAssetHome(domain)),
    [domain],
  );
  const linksAFile = useMemo(
    () =>
      plain !== undefined &&
      source !== undefined &&
      assetRefsIn(source, plain.resolve).length > 0,
    [plain, source],
  );
  const listing = useQuery(
    {
      queryKey: plain?.key ?? ["attachments", ""],
      queryFn: () => (plain as AssetHome).list(),
      // Without the app's provider there is nothing to fetch with.
      enabled: linksAFile && queryClient !== STANDALONE,
    },
    queryClient,
  );
  const versions = useMemo(
    () =>
      domain === undefined
        ? undefined
        : changedVersions(queryClient, `engram:${domain}`, listing.data),
    [queryClient, domain, listing.data],
  );
  return useMemo(
    () =>
      domain === undefined ? undefined : engramAssetHome(domain, versions),
    [domain, versions],
  );
}
