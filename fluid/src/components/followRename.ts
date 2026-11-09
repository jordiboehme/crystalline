/**
 * What follows a rename that went through, shared by the two places that ask
 * for one: the domain page's rename dialog and the MANIFEST editor's question
 * about a changed name.
 *
 * The cache is patched with what the report already says before the reader is
 * moved, the report waits in the query cache rather than in `location.state`,
 * and the navigation is a plain inline call. The reasons for all three are the
 * module doc of `RenameDomainDialog`, which this was lifted out of unchanged.
 */

import type { QueryClient } from "@tanstack/react-query";
import type { NavigateFunction } from "react-router";

import type { RenameReport } from "../api/admin";
import { renameReportKey } from "../api/admin";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import type { DomainListing } from "../api/domains";
import { engramsRoute } from "../paths";
import { refreshAfterWrite } from "../query/refreshAfterWrite";

/** Patch the listing, park the report, land on the renamed domain's engram view. */
export function followRename(
  queryClient: QueryClient,
  domain: string,
  report: RenameReport,
): void {
  queryClient.setQueryData(
    DOMAINS_QUERY_KEY,
    (current: DomainListing | undefined) => {
      if (current === undefined) {
        return current;
      }
      return {
        ...current,
        domains: current.domains.map((entry) =>
          entry.name === domain
            ? {
                ...entry,
                name: report.domain,
                canonicalName: report.domain,
                aliases: report.aliases,
                renaming: false,
              }
            : entry,
        ),
      };
    },
  );
  // Set before navigating: whichever navigation actually lands the reader on
  // this domain's page, the report is already waiting there.
  queryClient.setQueryData(renameReportKey(report.domain), report);
}

/** The engram view, plus the background true-up of the listing. */
export function landOnRenamed(
  queryClient: QueryClient,
  navigate: NavigateFunction,
  report: RenameReport,
): void {
  // The engram view, not the domain's address: only the engram view shows the
  // report, and the domain's address may open on the Reflection.
  void navigate(engramsRoute(report.domain), { replace: true });
  void refreshAfterWrite(queryClient, { queryKey: DOMAINS_QUERY_KEY });
}
