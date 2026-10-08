/**
 * The lint rule behind `src/base.ts`: a file other than that module may not
 * build an address that leaves the router, because under a path prefix such
 * an address would point at the root of the host.
 *
 * It refuses `/api/` literals, `window.open`, `globalThis.open`, the global
 * `open`, `location.assign`, `location.replace`, `location.origin`, an
 * assignment to `location.href`, and a WebSocket or EventSource opened at a
 * root path.
 *
 * Imported by `eslint.config.js` and tested by `src/basePathRule.test.ts`.
 * esquery's regex syntax takes no `/` inside a pattern, so a slash is
 * written `\x2F`.
 */
import type { Linter } from "eslint";

export const BASE_PATH_MESSAGE =
  "Build this address through src/base.ts (API_BASE, withBase, absoluteUrl, navigateTo, openWindow, openRoute), so it keeps working under a path prefix.";

const selectors = [
  "Literal[value=/^\\x2Fapi\\x2F/]",
  "TemplateElement[value.raw=/^\\x2Fapi\\x2F/]",
  "MemberExpression[object.name='window'][property.name='open']",
  "MemberExpression[object.name='globalThis'][property.name='open']",
  "MemberExpression[object.property.name='location'][property.name=/^(assign|replace|origin)$/]",
  "MemberExpression[object.name='location'][property.name=/^(assign|replace|origin)$/]",
  "AssignmentExpression > MemberExpression.left[property.name='href'][object.name='location']",
  "AssignmentExpression > MemberExpression.left[property.name='href'][object.property.name='location']",
  "NewExpression[callee.name=/^(WebSocket|EventSource)$/] > Literal.arguments[value=/^\\x2F/]",
  "NewExpression[callee.name=/^(WebSocket|EventSource)$/] > TemplateLiteral.arguments > TemplateElement:first-child[value.raw=/^\\x2F/]",
];

export const basePathRule: Linter.Config = {
  files: ["src/**/*.{ts,tsx}"],
  ignores: ["src/base.ts", "src/**/*.test.{ts,tsx}", "src/test/**"],
  rules: {
    "no-restricted-syntax": [
      "error",
      ...selectors.map((selector) => ({
        selector,
        message: BASE_PATH_MESSAGE,
      })),
    ],
    // The global `open`, which is `window.open`. A local function of that
    // name (the game's grid helpers) is a different binding and is left
    // alone, which a syntax selector could not tell apart.
    "no-restricted-globals": [
      "error",
      { name: "open", message: BASE_PATH_MESSAGE },
    ],
  },
};
