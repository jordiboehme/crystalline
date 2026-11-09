/**
 * The redo key suite (`redoKeySuite`) on Linux: CodeMirror and w3c-keyname read
 * `navigator.platform` once, when they load, so it is set before any import.
 */

import { vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "Linux x86_64",
    configurable: true,
  });
});

import { redoKeySuite } from "../test/redoKeySuite";

redoKeySuite();
