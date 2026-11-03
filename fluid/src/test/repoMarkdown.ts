/**
 * The repository's own Markdown as a test corpus: `docs/*.md`,
 * `release-notes/*.md` and `README.md`, read off disk. The unit suite runs
 * in Node, but this program has no Node types (it stays browser scoped), so
 * the two calls it needs are reached through `process.getBuiltinModule`
 * with a narrow type of their own. Vite's file allowance does not cover the
 * repository root, so a glob import cannot reach these folders.
 */

interface Fs {
  readdirSync: (path: string) => string[];
  readFileSync: (path: string, encoding: "utf8") => string;
}

function nodeProcess() {
  const node = globalThis as unknown as {
    process: { getBuiltinModule(id: string): unknown; cwd(): string };
  };
  return node.process;
}

/** Each file as `[path from the repository root, text]`, in a stable order. */
export function repoMarkdown(): [string, string][] {
  const process = nodeProcess();
  const { readdirSync, readFileSync } = process.getBuiltinModule(
    "node:fs",
  ) as Fs;
  // The suite runs from `fluid/`, one folder below the repository root.
  const root = `${process.cwd()}/../`;
  const out: [string, string][] = [];
  for (const dir of ["docs/", "release-notes/"]) {
    for (const name of readdirSync(root + dir).sort()) {
      if (name.endsWith(".md")) {
        out.push([dir + name, readFileSync(root + dir + name, "utf8")]);
      }
    }
  }
  out.push(["README.md", readFileSync(`${root}README.md`, "utf8")]);
  return out;
}
