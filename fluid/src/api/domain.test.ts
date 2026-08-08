/**
 * What a browse row is allowed to carry, read the way every list in this app
 * reads a row.
 *
 * The tree is the one source whose rows are navigation rather than description,
 * and the one field navigation cannot do without is `status`: it is what fades
 * a retired engram in a sidebar. The endpoint answers with it, so this pins
 * that it survives the read, and that a row without one still reads as a row
 * rather than throwing three components deep.
 */

import { describe, expect, it } from "vitest";

import { readTree } from "./domain";
import { defined } from "../test/assert";

/** One folder of a domain, in the shape the endpoint answers with. */
function browsePayload() {
  return {
    domain: "eng",
    path: "/",
    folders: ["notes"],
    engrams: [
      {
        permalink: "alpha",
        title: "Alpha",
        type: "engram",
        status: "stable",
        path: "alpha.md",
      },
      {
        permalink: "old",
        title: "Old Way",
        type: "decision",
        status: "deprecated",
        path: "old.md",
      },
    ],
  };
}

describe("a browse payload", () => {
  it("carries each engram's status into the row", () => {
    const tree = readTree(browsePayload(), "eng", "");

    expect(tree.folders).toEqual(["notes"]);
    expect(tree.engrams.map((row) => row.status)).toEqual([
      "stable",
      "deprecated",
    ]);
    // The domain of the request rides along, because a tree row names only
    // itself and a link out of one needs both halves of the address.
    const alpha = defined(tree.engrams[0], "the first row");
    expect(alpha.domain).toBe("eng");
    expect(alpha.type).toBe("engram");
  });

  it("leaves the status null when a row does not say", () => {
    const tree = readTree(
      { domain: "eng", path: "/", folders: [], engrams: [{ permalink: "a" }] },
      "eng",
      "",
    );

    // Null rather than a plausible default: a row claiming a state nobody
    // wrote would be a lie a reader cannot see through, and it would fade or
    // fail to fade on that lie.
    const row = defined(tree.engrams[0], "the first row");
    expect(row.status).toBeNull();
    expect(row.title).toBe("a");
  });
});
