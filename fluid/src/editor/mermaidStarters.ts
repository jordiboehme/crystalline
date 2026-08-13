/**
 * The diagram starters the toolbar's mermaid menu inserts: sixteen types in
 * three groups, each one a body a person can edit into their own diagram
 * rather than a syntax reminder they have to look up.
 *
 * Two rules hold every starter together. It PARSES under the pinned mermaid -
 * the sibling test runs the real parser over all sixteen, so a starter that
 * drifts out of the grammar fails the suite rather than the reader's editor.
 * And it names its editable token, the one word a person would replace first
 * (a title, a participant, the opening state), EXACTLY ONCE, so the caret can
 * land on it selected and one keystroke replaces the whole of it. The
 * once-only half of that rule is spelled out at `mermaidFence`.
 *
 * The bodies are line arrays, never joined strings: an insertion joins with
 * the buffer's own `state.lineBreak`, so a literal "\n" here would land as
 * line CONTENT in a CRLF document. `mermaidFence` wraps a body in its fence
 * and reports the token's place within the fenced block, which is the array
 * the toolbar hands to `insertBlock`.
 *
 * This module is data plus one wrapper: no mermaid import, so the sixteen
 * starters cost nothing until the menu that consumes them is opened. Its one
 * import is the toolbar's `selectToken` and the `BlockSelection` it returns,
 * which is where insertion lives anyway - a second copy of that search here
 * would be a second thing to keep in step with the caret it produces.
 */

import type { BlockSelection } from "./toolbar";
import { selectToken } from "./toolbar";

/** One insertable diagram: its menu label, its body and its first token. */
export interface MermaidStarter {
  label: string;
  lines: readonly string[];
  token: string;
}

/** A labelled run of starters, one section of the menu. */
export interface StarterGroup {
  label: string;
  starters: readonly MermaidStarter[];
}

/**
 * The flowchart body is byte-identical to the toolbar's `MERMAID_SKELETON`
 * body on purpose: keyboard-opening the menu highlights the first item, so
 * Enter Enter has to reproduce exactly what today's mermaid button inserts.
 * Only the caret differs - it now arrives selecting "First step".
 */
const EVERYDAY: readonly MermaidStarter[] = [
  {
    label: "Flowchart",
    lines: ["flowchart TD", "    A[First step] --> B[Next step]"],
    token: "First step",
  },
  {
    // Named participants rather than bare ones: an id used by two arrows is
    // written three times, and only the label is written once. See the
    // once-only rule at `mermaidFence`.
    label: "Sequence",
    lines: [
      "sequenceDiagram",
      "    participant caller as Caller",
      "    participant service as Service",
      "    caller->>service: Request",
      "    service-->>caller: Reply",
    ],
    token: "Caller",
  },
  {
    // Described states rather than bare ones, for the once-only rule: a state
    // that is entered and left names itself twice, its description once.
    label: "State",
    lines: [
      "stateDiagram-v2",
      '    state "First" as s1',
      '    state "Second" as s2',
      "    [*] --> s1",
      "    s1 --> s2 : event",
      "    s2 --> [*]",
    ],
    token: "First",
  },
  {
    // Labelled classes rather than bare ones, for the once-only rule: a class
    // that is declared and then related names itself twice, its label once.
    label: "Class",
    lines: [
      "classDiagram",
      '    class order["Order"] {',
      "        +String reference",
      "        +total() float",
      "    }",
      '    class item["Item"]',
      "    order --> item",
    ],
    token: "Order",
  },
  {
    label: "Entity relationship",
    lines: [
      "erDiagram",
      "    CUSTOMER ||--o{ ORDER : places",
      "    ORDER ||--|{ LINE-ITEM : contains",
    ],
    token: "CUSTOMER",
  },
  {
    label: "Gantt",
    lines: [
      "gantt",
      "    title Plan",
      "    dateFormat YYYY-MM-DD",
      "    section Delivery",
      "        First task :a1, 2026-01-06, 7d",
      "        Second task :after a1, 5d",
    ],
    token: "Plan",
  },
  {
    label: "Pie",
    lines: [
      "pie title Share",
      '    "First slice" : 60',
      '    "Second slice" : 40',
    ],
    token: "Share",
  },
];

const PLANNING: readonly MermaidStarter[] = [
  {
    label: "Timeline",
    lines: [
      "timeline",
      "    title Milestones",
      "    2026 Q1 : Kickoff",
      "    2026 Q2 : Launch",
    ],
    token: "Milestones",
  },
  {
    label: "User journey",
    lines: [
      "journey",
      "    title First visit",
      "    section Sign up",
      "        Find the site: 5: Visitor",
      "        Create an account: 3: Visitor",
    ],
    token: "First visit",
  },
  {
    label: "Quadrant chart",
    lines: [
      "quadrantChart",
      "    title Priorities",
      "    x-axis Low effort --> High effort",
      "    y-axis Low impact --> High impact",
      "    quadrant-1 Do now",
      "    quadrant-2 Schedule",
      "    quadrant-3 Drop",
      "    quadrant-4 Delegate",
      "    First idea: [0.7, 0.8]",
    ],
    token: "Priorities",
  },
  {
    label: "Mindmap",
    lines: [
      "mindmap",
      "    root((Central idea))",
      "        First branch",
      "            A detail",
      "        Second branch",
    ],
    token: "Central idea",
  },
];

const TECHNICAL: readonly MermaidStarter[] = [
  {
    label: "C4 context",
    lines: [
      "C4Context",
      "    title System context",
      '    Person(customer, "Customer", "A person with an account")',
      '    System(platform, "Platform", "Does the work")',
      '    Rel(customer, platform, "Uses")',
    ],
    token: "System context",
  },
  {
    label: "Requirement",
    lines: [
      "requirementDiagram",
      "    requirement first_requirement {",
      "        id: 1",
      "        text: The system shall do the thing",
      "        risk: medium",
      "        verifymethod: test",
      "    }",
      "    element first_check {",
      // Not "test case": an element type is lexed word by word, so a LEADING
      // keyword is rejected - any verifymethod (test, inspection, analysis,
      // demonstration) and any block field (id, text, risk). Only the first
      // word is constrained: "testbed" and "unit test" both parse.
      "        type: simulation",
      "    }",
      "    first_check - satisfies -> first_requirement",
    ],
    token: "The system shall do the thing",
  },
  {
    label: "Architecture",
    lines: [
      "architecture-beta",
      "    group platform(cloud)[Platform]",
      "    service api(server)[API] in platform",
      "    service store(database)[Store] in platform",
      "    api:R -- L:store",
    ],
    token: "Platform",
  },
  {
    label: "XY chart",
    lines: [
      "xychart-beta",
      '    title "Monthly total"',
      "    x-axis [jan, feb, mar]",
      '    y-axis "Amount" 0 --> 100',
      "    bar [30, 50, 80]",
      // Deliberately not the bar's own numbers: a line drawn on the bar tops
      // reads as a rendering artifact, not as a second series.
      "    line [20, 60, 70]",
    ],
    token: "Monthly total",
  },
  {
    label: "Radar",
    lines: [
      "radar-beta",
      "    title Team skills",
      '    axis s["Speed"], q["Quality"], c["Cost"]',
      '    curve now["Today"]{70, 80, 60}',
    ],
    token: "Team skills",
  },
];

/** The menu, in the order it is shown. */
export const MERMAID_STARTER_GROUPS: readonly StarterGroup[] = [
  { label: "Everyday", starters: EVERYDAY },
  { label: "Planning and product", starters: PLANNING },
  { label: "Technical", starters: TECHNICAL },
];

/**
 * Wrap a starter in its mermaid fence and say where its token sits in the
 * result: the lines go to `insertBlock`, the selection with them, so the
 * caret arrives on the first word worth replacing rather than at the end of
 * the fence line. Null when a starter carries no token in its body, which
 * leaves the insertion at its default caret.
 *
 * `selectToken` takes the FIRST occurrence, and every starter is written so
 * that it is also the ONLY one - the sibling test asserts exactly one mention
 * for all sixteen. That is the rule that makes typing over the selection a
 * finished edit rather than the first half of one: a token mentioned twice
 * leaves the second mention behind as a phantom state or a dangling edge, for
 * the person to find later in a diagram they thought they had renamed.
 *
 * Three diagram types need an identifier in more than one place, so their
 * starters separate the two jobs: a short id carries the structure and a
 * label carries the words. `participant caller as Caller`, `state "First" as
 * s1` and `class order["Order"]` each say their editable word once, and the
 * arrows underneath go on referring to the id. Any starter added later has
 * the same two ways out: mention the token once, or give it a label.
 */
export function mermaidFence(starter: MermaidStarter): {
  lines: string[];
  select: BlockSelection | null;
} {
  const lines = ["```mermaid", ...starter.lines, "```"];
  return { lines, select: selectToken(lines, starter.token) };
}
