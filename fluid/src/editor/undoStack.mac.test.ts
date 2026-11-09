/**
 * The redo key suite (`redoKeySuite`) on a Mac: CodeMirror and w3c-keyname read
 * `navigator.platform` once, when they load, so it is set before any import.
 */

import { vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import { redoKeySuite } from "../test/redoKeySuite";

redoKeySuite();
