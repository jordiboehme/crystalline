/**
 * One engram.
 *
 * A placeholder that names what the wildcard matched. The permalink is a path
 * with its own slashes, so it arrives in the splat param rather than a named
 * one; showing it here is what proves the route table carries it whole.
 */

import { useParams } from "react-router";

export default function EngramPage() {
  const params = useParams();
  const permalink = params["*"] ?? "";
  return (
    <h1 className="text-xl font-semibold">
      Engram: {permalink} in {params.domain}
    </h1>
  );
}
