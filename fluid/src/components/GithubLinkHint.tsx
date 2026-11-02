/**
 * What an account with no GitHub link is told on a domain GitHub governs:
 * that linking its GitHub account on its profile lets it contribute there,
 * and opens the private domains it can read on GitHub.
 *
 * Shown in two places: where the write button would be on a shared
 * governed domain (the account reads it as everyone signed in does, and
 * writes only once GitHub knows who it is), and on a private governed
 * domain's Reflection (which a Reflection collaborator reaches while the
 * engrams stay closed to it).
 */

import type { ReactElement } from "react";

import { UNLINKED_HINT } from "../githubLink";

export function GithubLinkHint(): ReactElement {
  return (
    <p className="text-sm text-slate-600 dark:text-slate-300">
      {UNLINKED_HINT}
    </p>
  );
}
