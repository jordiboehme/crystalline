/**
 * Whether the signed-in account has linked its GitHub account, for the hint
 * an unlinked account gets on a domain GitHub governs
 * (`components/GithubLinkHint.tsx`).
 */

import { useQuery } from "@tanstack/react-query";

import { MY_GITHUB_LINK_KEY, fetchMyGithubLink } from "./api/admin";
import { useAuth } from "./auth/AuthContext";

/** The sentence, in one place. */
export const UNLINKED_HINT =
  "Connect GitHub on your profile to contribute, and to open the private domains you can read on GitHub.";

/**
 * Whether the signed-in account has no GitHub link on an instance with an
 * App. Asks only when `governed` is true, and answers false until the read
 * lands or when it fails.
 */
export function useUnlinkedFromGithub(governed: boolean): boolean {
  const { user } = useAuth();
  const link = useQuery({
    queryKey: MY_GITHUB_LINK_KEY,
    queryFn: fetchMyGithubLink,
    enabled: governed && user !== null,
    retry: false,
  });
  return (
    governed &&
    link.data !== undefined &&
    link.data.appConfigured &&
    !link.data.linked
  );
}
