/**
 * What every test file gets before it runs: the DOM matchers, a clean document
 * between tests, and the handful of browser APIs jsdom does not implement but
 * the Radix primitives call into.
 */

import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
});

// jsdom implements neither the Pointer Events capture API nor
// `scrollIntoView`, and a Radix menu calls both while opening. They are
// no-ops here: the tests assert on what is in the document, never on where it
// scrolled to.
for (const name of [
  "hasPointerCapture",
  "setPointerCapture",
  "releasePointerCapture",
] as const) {
  if (!(name in Element.prototype)) {
    Object.defineProperty(Element.prototype, name, {
      value: () => false,
      writable: true,
    });
  }
}
if (!("scrollIntoView" in Element.prototype)) {
  Object.defineProperty(Element.prototype, "scrollIntoView", {
    value: () => undefined,
    writable: true,
  });
}

// jsdom performs no layout, so every element reports an `offsetHeight` of 0.
// A virtualized list measures its scrolling box that way and, told it is zero
// pixels tall, draws no rows at all. One nominal viewport for the whole suite
// is what makes those lists renderable here; the tests that care about the
// number set their own scroll offsets against it.
Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
  configurable: true,
  value: 600,
});

// Radix measures its content with a ResizeObserver, which jsdom does not have.
if (!("ResizeObserver" in globalThis)) {
  Object.defineProperty(globalThis, "ResizeObserver", {
    value: class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
    writable: true,
  });
}
