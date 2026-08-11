/**
 * The one mermaid configuration this app has.
 *
 * Two surfaces draw diagrams - the reading view's `MermaidDiagram` and the
 * editor's live fence preview - and a diagram must not change palette when
 * the same text moves from one to the other, so the configuration they hand
 * mermaid is this function rather than two literals that drift apart.
 *
 * `base` is the only built-in theme that honours `themeVariables`: `default`
 * and `dark` ignore them and draw in mermaid's own purple, beside an app that
 * is teal everywhere else. The variables below are the app's tokens by value,
 * taken from the accent and slate ramps in `index.css` - mermaid reads its
 * configuration as data before anything is in the document, so a CSS variable
 * would reach it as the literal string `var(--color-accent-700)`.
 *
 * `suppressErrorRendering` is not a preference. Mermaid's default on a parse
 * failure is to append its own error graphic to `document.body`, outside
 * React's tree and outside CodeMirror's, where nothing either surface does
 * can take it down again; the editor renders a half-typed diagram on every
 * keystroke, so those failures are the normal path. Both surfaces answer a
 * diagram that will not parse by showing the source instead.
 */

import type { MermaidConfig } from "mermaid";

/** The diagram palette per scheme: accent fills, slate lines, app text. */
const VARIABLES = {
  dark: {
    darkMode: true,
    background: "#0f172a",
    primaryColor: "#134e4a",
    primaryTextColor: "#e2e8f0",
    primaryBorderColor: "#2dd4bf",
    lineColor: "#64748b",
    secondaryColor: "#1e293b",
    tertiaryColor: "#0f172a",
    fontFamily: "ui-sans-serif, system-ui, sans-serif",
  },
  light: {
    background: "#ffffff",
    primaryColor: "#ccfbf1",
    primaryTextColor: "#0f172a",
    primaryBorderColor: "#0f766e",
    lineColor: "#475569",
    secondaryColor: "#f1f5f9",
    tertiaryColor: "#ffffff",
    fontFamily: "ui-sans-serif, system-ui, sans-serif",
  },
} as const;

/**
 * What both diagram surfaces pass to `mermaid.initialize`.
 *
 * `strict` is mermaid's own sanitizing mode: a diagram is drawn from text
 * somebody wrote into the knowledge base, and the labels in it are text.
 */
export function mermaidConfig(dark: boolean): MermaidConfig {
  return {
    startOnLoad: false,
    securityLevel: "strict",
    suppressErrorRendering: true,
    theme: "base",
    themeVariables: dark ? { ...VARIABLES.dark } : { ...VARIABLES.light },
  };
}
