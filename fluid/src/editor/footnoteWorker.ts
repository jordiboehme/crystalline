/**
 * The worker a long document's footnotes are read in: the text in, the
 * reference and definition offsets and the numbers out, for one version of
 * the document. See `footnotes.ts`.
 */

import { Text } from "@codemirror/state";

import { readNotes } from "./footnoteSyntax";

interface Scope {
  onmessage: ((event: { data: { id: number; text: string } }) => void) | null;
  postMessage(message: unknown): void;
}

const scope = self as unknown as Scope;

scope.onmessage = (event) => {
  const { id, text } = event.data;
  scope.postMessage({ id, notes: readNotes(Text.of(text.split("\n"))) });
};
