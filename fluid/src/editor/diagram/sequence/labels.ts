/**
 * What a sequence text is written as, so that what a person types is what is
 * drawn. mermaid decodes the entity forms in message, note, alias and block
 * text, so every character that would be changed on the way gets its form:
 * - `#` cuts the text (R55) and `;` ends the statement
 * - `<` and `>` are markup to `sanitizeText`, `&` starts an entity to it
 * - U+FB02 and U+00B6 start `decodeEntities`' markers
 * - a leading `wrap:` or `nowrap:` is cut by `parseMessage`
 * - `%%{` is taken out as a directive
 * - two `$$` make a formula, drawn by another renderer
 * - `style` or `classDef` with a `:` and then a `#` loses the line's last `;` in
 *   `encodeEntities`, which the forms themselves would meet, so there the `:` is a
 *   form too.
 *
 * Each was seen in Chromium with the app's `mermaidConfig`.
 */

import { decodeDrawn } from "../../../theme/mermaidText";
import { losesSemicolon } from "./parse";

const FORMS: Readonly<Record<string, string>> = {
  "#": "#35;",
  ";": "#59;",
  "<": "#lt;",
  ">": "#gt;",
  "&": "#38;",
  "\n": "<br>",
  ﬂ: "#64258;",
  "\xB6": "#182;",
};

/** The text as the check compares it: one line break form, no outer space. */
export function tidySeqText(text: string): string {
  return text.replace(/\r\n?/g, "\n").trim();
}

/** `text` as the source writes it: it decodes back to `tidySeqText(text)`. */
export function encodeSeqText(text: string): string {
  let out = tidySeqText(text)
    .replace(/[#;<>&\nﬂ\xB6]/g, (c) => FORMS[c] ?? c)
    .replace(/%%\{/g, "#37;#37;{")
    .replace(/^(:?(?:no)?wrap):/, "$1#58;");
  if (/\$\$.*\$\$/.test(out)) {
    out = out.replaceAll("$", "#36;");
  }
  return losesSemicolon(out) ? out.replaceAll(":", "#58;") : out;
}

/**
 * `encodeSeqText` for the inside of a quoted `@{ alias }`: there `"` and `\` belong
 * to the string and `}` ends the config.
 */
export function encodeConfigAlias(text: string): string {
  return encodeSeqText(text)
    .replaceAll('"', "#quot;")
    .replaceAll("\\", "#92;")
    .replaceAll("}", "#125;");
}

/**
 * Whether `drawnLabels` would change this text once it stands in the drawing. It
 * decodes `&word;` and `&#n;` in the drawn text, and by then a typed `&amp;` is the
 * same text as a form, so no way of writing it draws as typed.
 */
export function drawsOtherwise(text: string): boolean {
  return decodeDrawn(text) !== text;
}
