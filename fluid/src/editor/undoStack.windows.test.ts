/**
 * The redo key suite (`redoKeySuite`) on Windows: CodeMirror and w3c-keyname read
 * `navigator.platform` once, when they load, so it is set before any import.
 */

import { vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "Win32",
    configurable: true,
  });
});

import { redoKeySuite } from "../test/redoKeySuite";

redoKeySuite();
