/**
 * The lint rule behind `src/base.ts`: a file other than that module may not
 * build an address that leaves the router, because under a path prefix such
 * an address would point at the root of the host.
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
  "MemberExpression[object.property.name='location'][property.name='assign']",
  "MemberExpression[object.name='location'][property.name='assign']",
  "MemberExpression[object.property.name='location'][property.name='origin']",
  "MemberExpression[object.name='location'][property.name='origin']",
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
  },
};
