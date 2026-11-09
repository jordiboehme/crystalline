/**
 * A small drawing of a sequence body with the parts mermaid 12.2.0 marks, for tests
 * that stub mermaid's render: per participant a life line and a top box with
 * `data-et` and `data-id`, per message its text and then its line with
 * `data-id="i<record>"`, `data-from` and `data-to`, as the recorded drawings have
 * them. Read from the parser, so it draws what the source says.
 */

import { decodeSeqText, parseSequence } from "../editor/diagram/sequence/parse";

const escape = (text: string) =>
  text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");

/** Whether `body` reads as a sequence diagram. */
export function isSequenceBody(body: string): boolean {
  return (
    body.includes("sequenceDiagram") &&
    parseSequence(body).scan.keyword === "sequenceDiagram"
  );
}

export function sequenceDrawing(id: string, body: string): string {
  const model = parseSequence(body);
  const people = model.order
    .map((name, at) => {
      const x = String(75 + 200 * at);
      const label = model.participants.get(name)?.label ?? name;
      return (
        `<g><line x1="${x}" y1="65" x2="${x}" y2="300" data-et="life-line" data-id="${escape(name)}"></line>` +
        `<g data-et="participant" data-type="participant" data-id="${escape(name)}">` +
        `<rect x="${String(200 * at)}" y="0" width="150" height="65" class="actor actor-top"></rect>` +
        `<text x="${x}" y="32.5" class="actor actor-box"><tspan x="${x}">${escape(label)}</tspan></text></g></g>`
      );
    })
    .join("");
  const messages = model.messages
    .map((message, at) => {
      const text =
        message.text === null
          ? ""
          : decodeSeqText(body.slice(message.text.from, message.text.to));
      const y = String(100 + 46 * at);
      return (
        `<text x="100" y="${y}" class="messageText">${escape(text)}</text>` +
        `<line x1="76" y1="${y}" x2="271" y2="${y}" class="messageLine0" data-et="message" data-id="i${String(message.record)}" data-from="${escape(message.from)}" data-to="${escape(message.to)}"></line>`
      );
    })
    .join("");
  return `<svg id="${id}" aria-roledescription="sequence">${people}${messages}</svg>`;
}
