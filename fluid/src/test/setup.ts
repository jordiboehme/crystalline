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

// Node ships its own global `localStorage`/`sessionStorage`. vitest's jsdom
// environment only overrides a global that already exists on `globalThis`
// when the name is on its fixed list of window properties, and storage is
// not on it, so Node's copy - unusable without `--localstorage-file` - wins
// over jsdom's real one. `globalThis.jsdom` is the live `JSDOM` instance the
// environment stashed there, and its `window` carries the storage jsdom
// actually implements; repointing the two globals at it is what makes
// `localStorage` behave like a browser's inside every test.
const jsdomWindow = (globalThis as { jsdom?: { window: Window } }).jsdom
  ?.window;
if (jsdomWindow) {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    get: () => jsdomWindow.localStorage,
  });
  Object.defineProperty(globalThis, "sessionStorage", {
    configurable: true,
    get: () => jsdomWindow.sessionStorage,
  });
}

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

// CodeMirror measures text through Ranges, which jsdom leaves layout-less.
// Zero-size answers are enough: the tests assert on documents and DOM
// structure, never on pixel geometry.
const zeroRect = {
  x: 0,
  y: 0,
  top: 0,
  bottom: 0,
  left: 0,
  right: 0,
  width: 0,
  height: 0,
  toJSON: () => ({}),
} as DOMRect;
Range.prototype.getBoundingClientRect = () => zeroRect;
Range.prototype.getClientRects = () =>
  ({
    length: 0,
    item: () => null,
    *[Symbol.iterator]() {},
  }) as unknown as DOMRectList;

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
