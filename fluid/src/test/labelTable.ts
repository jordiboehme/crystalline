/**
 * The labels at the edges of mermaid's grammar, each on every way a box can
 * hold a label. Two suites run this one table: `opsProperty.test.ts` to mermaid's
 * db, `e2e/mermaid-labels.spec.ts` to what is drawn in a browser.
 */

/** A box's ref, as written for the id `A`. */
export const LABEL_HOLDERS: Readonly<Record<string, string>> = {
  rectangle: "A[Old]",
  rounded: "A(Old)",
  flag: "A>Old]",
  decision: "A{Old}",
  lean: "A[/Old/]",
  quoted: 'A["Old"]',
  markdown: 'A["`Old`"]',
  bare: "A",
  at: "A@{ shape: diam, label: Old }",
  atQuoted: 'A@{ shape: diam, label: "Old" }',
};

/** The texts `rename` refuses on every holder. */
export const REFUSED_LABELS: readonly string[] = [
  // R7: a line holding this is a direction statement.
  "go direction LR",
  "direction LR",
  // R42: mermaid takes it out of the text as a directive.
  "%%{init}%%",
  "a %%{b: c",
  // With a line break in the label its text is parsed as HTML once more, so an
  // entity form is read one level deeper and nothing can show these as typed.
  "x &amp; y\nz",
  "&quot;\nz",
  "&#35;\nz",
  "C:\\new\nz",
  "\u00B6\u00DF\nz",
  "\uFB02\u00B0\nz",
];

/**
 * R42: on a plain label mermaid drops this `;`, with more text after it. The
 * markdown and `@{ }` holders write the `:` as an entity form, so they apply it.
 */
export const REFUSED_ON_PLAIN_LABELS: readonly string[] = [
  "style a:#b; c",
  'style:"x"; y',
  "classDef:#x; y",
];

/**
 * Text that is markup in a markdown label, where the person wrote markup and it
 * draws as such (the `markdown` holder only).
 */
export const MARKDOWN_MARKUP: readonly string[] = [
  "_a_",
  "*a*",
  "**b**",
  "__b__",
];

/** The texts that apply on every holder and draw as typed. */
export const LABEL_TEXTS: readonly string[] = [
  "a/",
  "/a",
  "-a",
  "a-",
  "\\a",
  "a\\",
  "x\\ny",
  "C:\\new",
  "end",
  "graph",
  "subgraph",
  "style x",
  "click",
  "a;b",
  "a#b",
  "#35;",
  "#quot;",
  "&amp;",
  "&quot;",
  "&#35;",
  "#a_b;",
  "x #_; y",
  "a:#b;",
  "a`b",
  "`a`",
  "a~~b",
  "a--b",
  "a-->b",
  "x > y",
  "> q",
  "a<b",
  "x < y",
  "me@host",
  "a:b",
  "a,b",
  "100%",
  "%%",
  "%% c",
  "a%%b",
  "<b>x</b>",
  "<script>x</script>",
  "<!--c-->",
  "<img src=x onerror=y>",
  "A & B",
  'say "hi"',
  "direction",
  "1",
  "true",
  "null",
  "a\tb",
  "K\u00F6ln",
  "a  b",
  "one\ntwo",
  "A & B\nC",
  "a&b\nc",
  "&\n&",
  "#quot;\nz",
  "#9829;\nz",
  "<b>\n</b>",
  "<script>x</script>\nz",
  "a`b\nz",
  "<br>\nz",
  'AT&T\n<b> & "q"',
  " a",
  "a ",
  "'a'",
  "a=b",
  "_a_",
  "*a*",
  "@{x}",
  "a<br>b",
  "|",
  "...",
  "\u00B6\u00DF",
  "\uFB02\u00B0",
];

/** Every ASCII punctuation mark at each place in a word. */
export const PUNCTUATION_TEXTS: readonly string[] = [
  ..."!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~",
].flatMap((c) => [`a${c}b`, `${c}a`, `a${c}`, `a ${c} b`]);

/** What markdown reads as blocks or inline marks: only `@{ }` and backtick labels go there. */
export const MARKDOWN_TEXTS: readonly string[] = [
  "- x",
  "* x",
  "+ x",
  "1. x",
  "# x",
  "## x",
  "---",
  "===",
  "**b**",
  "__b__",
  "[l](u)",
  "![i](u)",
  "http://x.y",
  "a_b_c",
  "| t |",
  "~~s~~",
  "www.x.y",
  "me@host.de",
  "a: b",
  "[y]",
  "x [y] z",
];

/**
 * Labels as somebody wrote them by hand, between quotes. The rename field must
 * show what each one draws, whatever way it was written (R45).
 */
export const HAND_WRITTEN_LABELS: readonly string[] = [
  "#quot;",
  "#35;quot;",
  "#38;quot;",
  "#amp;quot;",
  "&amp;quot;",
  "&amp;lt;b&amp;gt;",
  "&amp;",
  "&amp;amp;",
  "#amp;",
  "#amp;amp;",
  "#lt;b#gt;",
  "&lt;b&gt;",
  "#9829;",
  "#infin;",
  "#bogus;",
  "&bogus;",
  "R&D;x",
  "A & B",
  "a#b",
  "#35;",
  "#92;",
];
