/**
 * One domain.
 *
 * A placeholder that names the domain the route matched, so the route table
 * can be checked without the screen existing yet. The manifest, the tree and
 * the engram list arrive with the task that builds them.
 */

import { useParams } from "react-router";

export default function DomainHome() {
  const { domain } = useParams();
  return <h1 className="text-xl font-semibold">Domain: {domain}</h1>;
}
