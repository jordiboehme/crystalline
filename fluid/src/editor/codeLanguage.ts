/**
 * The language a fence's info word names, in the one place it is asked: the
 * editor's highlighting (`documentLanguage` in `setup.ts`), the code box's
 * header and its language menu all read it here, so they never disagree.
 *
 * The match is exact on a language's name or one of its aliases, case
 * aside. No fuzzy match: it found the alias `tex` inside `text`,
 * `plaintext` and `context` and called them LaTeX. The usual words for
 * plain text name no language.
 */

import { LanguageDescription } from "@codemirror/language";
import { languages } from "@codemirror/language-data";

/** Words that say "plain text" and name no language. */
const PLAIN_TEXT = new Set(["text", "plaintext", "plain", "txt"]);

/**
 * Words with no entry of their own that name a known language: what the old
 * fuzzy match found right (`json` inside them) and the exact one would
 * miss. `codeBlocks.test.ts` compares both lookups over every name and alias
 * and pins this list.
 */
const ALSO = new Map([
  ["jsonc", "json"],
  ["jsonl", "json"],
  ["ndjson", "json"],
]);

/** The language `word` names, or null for none. */
export function languageOf(word: string | null): LanguageDescription | null {
  if (word === null || word === "") {
    return null;
  }
  const lower = word.toLowerCase();
  if (PLAIN_TEXT.has(lower)) {
    return null;
  }
  return LanguageDescription.matchLanguageName(
    languages,
    ALSO.get(lower) ?? lower,
    false,
  );
}

/**
 * The highlighting's lookup for a fence's info: its first word, as the code
 * box reads it.
 */
export function codeLanguageFor(info: string): LanguageDescription | null {
  return languageOf(/^\S*/.exec(info.trimStart())?.[0] ?? "");
}
