/**
 * The format commands with a peer typing in the same room: a format waiting
 * for typing keeps its place through a peer's insert, and a peer's letter at
 * the same place as ours is never lost on either side.
 */

import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  keys,
  markPeers,
  setCaret,
  typeText,
} from "../../test/markKit";
import { charFormats } from "./formatting";

afterEach(cleanupMarkViews);

describe("two peers", () => {
  it("Mod-b waits through a peer's insert at the caret: c is bold, Z is not", () => {
    const { local, peer, sync } = markPeers("ab");
    setCaret(local, 2);
    keys(local, "Mod-b");
    setCaret(peer, 2);
    typeText(peer, "Z");
    sync();
    typeText(local, "c");
    sync();
    const text = bytes(local.state);
    expect(bytes(peer.state)).toBe(text);
    expect(text).toContain("Z");
    expect(text).toContain("c");
    expect([...charFormats(local.state, text.indexOf("c"))]).toEqual([
      "strong",
    ]);
    expect([...charFormats(local.state, text.indexOf("Z"))]).toEqual([]);
  });

  it("both letters typed at the end of a bold word survive in both peers", () => {
    const { local, peer, sync } = markPeers("a **bold** c");
    setCaret(local, 8);
    setCaret(peer, 8);
    typeText(local, "x");
    typeText(peer, "y");
    sync();
    const text = bytes(local.state);
    expect(bytes(peer.state)).toBe(text);
    expect(text).toContain("x");
    expect(text).toContain("y");
  });
});
