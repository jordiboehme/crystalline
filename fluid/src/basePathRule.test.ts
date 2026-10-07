/**
 * The lint rule that keeps every address leaving the router going through
 * `src/base.ts`. Node-context code like `src/bundleBudget.test.ts`: it
 * imports `scripts/basePathRule.ts` directly, so it typechecks under
 * `tsconfig.node-test.json`.
 */
import { Linter } from "eslint";
import tseslint from "typescript-eslint";
import { describe, expect, it } from "vitest";

import { BASE_PATH_MESSAGE, basePathRule } from "../scripts/basePathRule.ts";

const linter = new Linter({ configType: "flat" });

function messages(code: string, filename = "src/screens/Example.tsx") {
  return linter
    .verify(
      code,
      [
        {
          files: ["**/*.{ts,tsx}"],
          languageOptions: { parser: tseslint.parser },
        },
        basePathRule,
      ],
      filename,
    )
    .map((m) => m.message);
}

describe("the base path lint rule", () => {
  it.each([
    'fetch("/api/v1/domains");',
    "fetch(`/api/v1/domains/${d}`);",
    'window.location.assign("/login");',
    "location.assign(next);",
    'window.open("/d/x", "_blank");',
    "const link = `${window.location.origin}/d/x`;",
    "const o = location.origin;",
  ])("refuses %s", (code) => {
    expect(messages(code)).toContain(BASE_PATH_MESSAGE);
  });

  it.each([
    "const a = withBase('/d/x');",
    'const b = "/d/x";',
    "const c = API_BASE + '/domains';",
    "const p = window.location.pathname;",
  ])("allows %s", (code) => {
    expect(messages(code)).toEqual([]);
  });

  it("leaves base.ts and test files alone", () => {
    expect(messages('fetch("/api/v1/x");', "src/base.ts")).toEqual([]);
    expect(messages('fetch("/api/v1/x");', "src/api/client.test.ts")).toEqual(
      [],
    );
  });
});
