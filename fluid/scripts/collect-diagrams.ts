/**
 * Collects mermaid diagrams out of a folder of Markdown files, for the
 * opt-in real corpus the diagram property suites run over when
 * `FLUID_DIAGRAM_CORPUS` names a folder.
 *
 * Walks `<folder>` recursively for `.md` files, copies the body of every
 * fence whose opening line is exactly three backticks followed by `mermaid`
 * into `<out>/<n>-<file stem>.mmd` (`n` counts up from 1 across the whole
 * run), and prints how many bodies start with each first keyword. The body is
 * copied byte for byte, line breaks included; nothing is cleaned up, since a
 * corpus is only worth having in the shape people wrote it.
 *
 * The output folder stays on the machine it was collected on: it is never
 * committed (the committed corpus is hand-written, see
 * `src/editor/fixtures/diagrams/`).
 *
 * Usage (from `fluid/`): `node scripts/collect-diagrams.ts <folder> <out>`.
 */

import {
  mkdirSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, extname, join } from "node:path";

const FENCE_OPEN = "```mermaid";

function markdownFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      out.push(...markdownFiles(path));
    } else if (extname(name) === ".md") {
      out.push(path);
    }
  }
  return out;
}

/** Each mermaid fence body in `text`, with the line breaks the file uses. */
function mermaidBodies(text: string): string[] {
  const lines = text.split(/(?<=\n)/);
  const bodies: string[] = [];
  let body: string[] | undefined;
  for (const line of lines) {
    const bare = line.replace(/\r?\n$/, "");
    if (body === undefined) {
      if (bare === FENCE_OPEN) {
        body = [];
      }
    } else if (/^```\s*$/.test(bare)) {
      bodies.push(body.join(""));
      body = undefined;
    } else {
      body.push(line);
    }
  }
  return bodies;
}

/** The first word of the first line that is not blank, a comment or frontmatter. */
function firstKeyword(body: string): string {
  let inFrontmatter = false;
  let first = true;
  for (const raw of body.split(/\r?\n/)) {
    const line = raw.trim();
    if (first && line === "---") {
      inFrontmatter = true;
      first = false;
      continue;
    }
    first = false;
    if (inFrontmatter) {
      if (line === "---") {
        inFrontmatter = false;
      }
      continue;
    }
    if (line === "" || line.startsWith("%%")) {
      continue;
    }
    return line.split(/\s+/)[0] ?? "";
  }
  return "(empty)";
}

function main(): void {
  const [folder, out] = process.argv.slice(2);
  if (folder === undefined || out === undefined) {
    console.error("usage: node scripts/collect-diagrams.ts <folder> <out>");
    process.exit(2);
  }
  mkdirSync(out, { recursive: true });
  const counts = new Map<string, number>();
  let n = 0;
  for (const file of markdownFiles(folder)) {
    const stem = basename(file, ".md");
    for (const body of mermaidBodies(readFileSync(file, "utf8"))) {
      n += 1;
      writeFileSync(join(out, `${String(n)}-${stem}.mmd`), body);
      const keyword = firstKeyword(body);
      counts.set(keyword, (counts.get(keyword) ?? 0) + 1);
    }
  }
  console.log(`${String(n)} diagrams written to ${out}`);
  for (const [keyword, count] of [...counts].sort((a, b) => b[1] - a[1])) {
    console.log(`  ${keyword}: ${String(count)}`);
  }
}

main();
