/**
 * The one mermaid configuration this app has.
 *
 * Two surfaces draw diagrams - the reading view's `MermaidDiagram` and the
 * editor's live fence preview - and a diagram must not change palette when
 * the same text moves from one to the other, so the configuration they hand
 * mermaid is this function rather than two literals that drift apart.
 *
 * `base` is the only built-in theme that honours `themeVariables`: `default`
 * and `dark` ignore them and draw in mermaid's own purple, a different shade
 * than the app's own accent. The variables below are the app's tokens by
 * value, taken from the accent and slate ramps in `index.css` - mermaid
 * reads its configuration as data before anything is in the document, so a
 * CSS variable would reach it as the literal string `var(--color-accent-700)`.
 *
 * `suppressErrorRendering` is not a preference. Mermaid's default on a parse
 * failure is to append its own error graphic to `document.body`, outside
 * React's tree and outside CodeMirror's, where nothing either surface does
 * can take it down again; the editor renders a half-typed diagram on every
 * keystroke, so those failures are the normal path. Both surfaces answer a
 * diagram that will not parse by showing the source instead, and the editor's
 * preview adds one muted line saying why it is blank (`describeMermaidError`
 * in `editor/fencePreviews`) - never mermaid's own graphic.
 */

import type { MermaidConfig } from "mermaid";

/**
 * The diagram palette per scheme: accent fills, slate lines, app text.
 *
 * Four of these are here because `base` DERIVES everything it is not told.
 * Its theme constructor sets `noteBkgColor: "#fff5ad"` and `noteTextColor:
 * "#333"` outright, so the `||` fallbacks further down never fire and a
 * sequence or class diagram's notes come out in mermaid's yellow in BOTH
 * schemes - a highlighter box on a slate page. And `titleColor` falls back to
 * `tertiaryTextColor`, which is the channel inversion of `tertiaryColor`:
 * inverting the dark scheme's `#0d0f16` gives the warm cream `#f2f0e9`, so
 * subgraph and cluster titles would be the one thing on the page that is not
 * accent or slate. `arrowheadColor` derives the same way `titleColor` does, but
 * more directly - `this.arrowheadColor = this.arrowheadColor ||
 * invert_default(this.background)` inverts `background` itself rather than
 * routing through `tertiaryColor`, landing on that same warm cream for the
 * dark scheme's `#0d0f16`, so every arrowhead in a flowchart or user journey
 * would read as the one non-accent, non-slate mark on the page - the wart this
 * file pins shut by naming it to match `lineColor` per scheme. Naming all
 * four is what closes that: overrides are applied again AFTER the derivation
 * pass, so a named variable always wins.
 *
 * gitGraph's commit labels and mindmap's section colours are the same wart
 * in two more places, and they close the same way, by name. `base` derives
 * `git0..git7` and `gitBranchLabel0..7` from the primary colour and leaves
 * the branch label text black, and `commitLabelColor` is the channel inversion
 * of `commitLabelBackground`; the mindmap reads `cScale0..11` (a node's
 * fill), `cScaleLabel0..11` (its text, black on the root) and `cScaleInv0..11`
 * (its edges, black on the dark canvas). All of them are named from `RAMP`
 * below, one set per scheme, so a git graph or a mindmap in this app is drawn
 * in the same purple, blue and slate as everything else. Nothing here is a
 * per-diagram directive: they are theme variables, so the reading view, the
 * editor preview and the paper copy all get them.
 *
 * C4 is closed with `themeCSS`, not a variable. Its renderer writes the
 * relationship text, the boundary text and the relationship line as SVG
 * presentation attributes with a hard-coded `#444444` (unless the diagram
 * source says `UpdateRelStyle`), which no theme variable reaches. CSS beats a
 * presentation attribute, so `c4Css` below re-points exactly those attribute
 * values at the scheme's ink and line colours; inline-styled text and
 * `UpdateRelStyle` colours are untouched.
 */
/**
 * Eight series colours per scheme, each dark enough to carry white label text
 * (5:1 or better) and far enough from its canvas to read as a line (3:1).
 * Purple first, because the accent is the one colour every diagram already
 * has.
 */
const RAMP = {
  dark: [
    "#6d5bd0",
    "#3a6fb5",
    "#25786d",
    "#3f7a33",
    "#8f5f0f",
    "#b0393c",
    "#9a4283",
    "#52627a",
  ],
  light: [
    "#45388c",
    "#2b5797",
    "#0f766e",
    "#2f6b25",
    "#92400e",
    "#b91c1c",
    "#9d174d",
    "#475569",
  ],
} as const;

/** Text that sits on a `RAMP` fill, in either scheme. */
const ON_RAMP = "#f8fafc";

/** The ramp's series variables: the git graph's eight and the mindmap's twelve. */
function seriesVariables(ramp: readonly string[]): Record<string, string> {
  const out: Record<string, string> = {};
  const colour = (index: number) => ramp[index % ramp.length] ?? ON_RAMP;
  for (let i = 0; i < 8; i += 1) {
    out[`git${String(i)}`] = colour(i);
    out[`gitBranchLabel${String(i)}`] = ON_RAMP;
  }
  for (let i = 0; i < 12; i += 1) {
    out[`cScale${String(i)}`] = colour(i);
    out[`cScaleLabel${String(i)}`] = ON_RAMP;
    out[`cScaleInv${String(i)}`] = colour(i);
  }
  return out;
}

/**
 * The CSS that re-points C4's hard-coded `#444444` text and line colours at
 * the scheme's own ink and line (mermaid scopes `themeCSS` under the diagram's
 * id, and only an attribute selector reaches these elements).
 */
function c4Css(scheme: "dark" | "light"): string {
  const { primaryTextColor, lineColor } = VARIABLES[scheme];
  return `text[fill="#444444"] { fill: ${primaryTextColor}; } path[stroke="#444444"], line[stroke="#444444"] { stroke: ${lineColor}; }`;
}

const VARIABLES = {
  dark: {
    darkMode: true,
    background: "#0d0f16",
    primaryColor: "#2a1f5f",
    primaryTextColor: "#e2e8f0",
    primaryBorderColor: "#978bd3",
    lineColor: "#64748b",
    arrowheadColor: "#64748b",
    secondaryColor: "#1e293b",
    tertiaryColor: "#0d0f16",
    noteBkgColor: "#1e293b",
    noteTextColor: "#e2e8f0",
    titleColor: "#e2e8f0",
    commitLabelColor: "#e2e8f0",
    commitLabelBackground: "#1e293b",
    fontFamily: "ui-sans-serif, system-ui, sans-serif",
  },
  light: {
    background: "#ffffff",
    primaryColor: "#ece8f9",
    primaryTextColor: "#0f172a",
    primaryBorderColor: "#45388c",
    lineColor: "#475569",
    arrowheadColor: "#475569",
    secondaryColor: "#f1f5f9",
    tertiaryColor: "#ffffff",
    noteBkgColor: "#f1f5f9",
    noteTextColor: "#0f172a",
    titleColor: "#0f172a",
    commitLabelColor: "#0f172a",
    commitLabelBackground: "#f1f5f9",
    fontFamily: "ui-sans-serif, system-ui, sans-serif",
  },
} as const;

/**
 * What both diagram surfaces pass to `mermaid.initialize`.
 *
 * `strict` is mermaid's own sanitizing mode: a diagram is drawn from text
 * somebody wrote into the knowledge base, and the labels in it are text.
 *
 * `look` and `layout` are here because mermaid 12 moved both defaults, and a
 * default that moves once will move again. The new `neo` look paints node
 * strokes with a gradient on a theme that sets `useGradient` - which `base`,
 * the only theme that honours the variables above, does - so every diagram
 * already in the knowledge base would have quietly stopped drawing in the
 * accent border this module names. The new default layout engine is ELK,
 * which re-lays out flowchart, state, class, ER, requirement and use-case
 * diagrams: a drawing somebody arranged and then wrote about in the prose
 * beside it would come back arranged differently, and it would arrive with
 * ELK's own chunk on the diagram path. Both are named rather than inherited
 * so what an engram drew yesterday is what it draws today; adopting either
 * new default is a decision to take deliberately, with the engrams in view.
 *
 * `htmlLabels` is off at the root (mermaid 12 reads the root key; the
 * per-diagram ones are deprecated) because an HTML label sits in a
 * foreignObject whose size mermaid measures before Safari applies its page
 * zoom: under zoom the text overflows the node it belongs to. SVG text scales
 * with its node. `wrappingWidth` is wide because SVG labels wrap at it, and a
 * long identifier broken across two lines is no longer the identifier. SVG
 * text leaves entity forms undecoded, so everything `mermaid.render` draws goes
 * through `drawnLabels` (`mermaidText.ts`).
 */
export function mermaidConfig(dark: boolean): MermaidConfig {
  return {
    startOnLoad: false,
    securityLevel: "strict",
    suppressErrorRendering: true,
    theme: "base",
    look: "classic",
    layout: "dagre",
    htmlLabels: false,
    flowchart: { wrappingWidth: 2000 },
    themeCSS: c4Css(dark ? "dark" : "light"),
    themeVariables: dark
      ? { ...VARIABLES.dark, ...seriesVariables(RAMP.dark) }
      : { ...VARIABLES.light, ...seriesVariables(RAMP.light) },
  };
}
