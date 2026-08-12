/**
 * The diagram starters the toolbar's mermaid menu inserts: sixteen types in
 * three groups, each one a body a person can edit into their own diagram
 * rather than a syntax reminder they have to look up.
 *
 * Two rules hold every starter together. It PARSES under the pinned mermaid -
 * the sibling test runs the real parser over all sixteen, so a starter that
 * drifts out of the grammar fails the suite rather than the reader's editor.
 * And it names its first editable token, the one word a person would replace
 * first (a title, a participant, the opening state), so the caret can land on
 * it selected and the first keystroke after the insert is already content.
 *
 * The bodies are line arrays, never joined strings: an insertion joins with
 * the buffer's own `state.lineBreak`, so a literal "\n" here would land as
 * line CONTENT in a CRLF document. `mermaidFence` wraps a body in its fence
 * and reports the token's place within the fenced block, which is the array
 * the toolbar hands to `insertBlock`.
 *
 * This module is data plus one pure function: no CodeMirror import, no
 * mermaid import. It costs nothing until the menu that consumes it is opened.
 */

/**
 * Where the caret lands inside a freshly inserted block: an index into the
 * inserted line array plus a character range within that line.
 *
 * Defined here rather than imported because the toolbar's own copy (with its
 * `selectToken` helper and the `insertBlock` parameter that takes it) lands in
 * a later task; the shape is the plan's, so the two are the same type and one
 * import replaces this declaration when the toolbar side exists.
 */
export interface BlockSelection {
  line: number;
  from: number;
  to: number;
}

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
    label: "Sequence",
    lines: [
      "sequenceDiagram",
      "    participant Caller",
      "    participant Service",
      "    Caller->>Service: Request",
      "    Service-->>Caller: Reply",
    ],
    token: "Caller",
  },
  {
    label: "State",
    lines: [
      "stateDiagram-v2",
      "    [*] --> First",
      "    First --> Second : event",
      "    Second --> [*]",
    ],
    token: "First",
  },
  {
    label: "Class",
    lines: [
      "classDiagram",
      "    class Order {",
      "        +String reference",
      "        +total() float",
      "    }",
      "    Order --> Item",
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
      // Not "test case": the grammar lexes `test` as the verifymethod keyword
      // wherever a value is expected, so an element type has to avoid it.
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
      "    line [30, 50, 80]",
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
 * Find the FIRST occurrence of a token across a block's lines.
 *
 * First occurrence rather than exactly-once by design: a state diagram
 * necessarily repeats a state name, and every starter is written so its
 * first mention is the one a person edits.
 */
function firstOccurrence(
  lines: readonly string[],
  token: string,
): BlockSelection | null {
  for (const [line, text] of lines.entries()) {
    const from = text.indexOf(token);
    if (from !== -1) {
      return { line, from, to: from + token.length };
    }
  }
  return null;
}

/**
 * Wrap a starter in its mermaid fence and say where its token sits in the
 * result: the lines go to `insertBlock`, the selection with them, so the
 * caret arrives on the first word worth replacing rather than at the end of
 * the fence line. Null when a starter carries no token in its body, which
 * leaves the insertion at its default caret.
 */
export function mermaidFence(starter: MermaidStarter): {
  lines: string[];
  select: BlockSelection | null;
} {
  const lines = ["```mermaid", ...starter.lines, "```"];
  return { lines, select: firstOccurrence(lines, starter.token) };
}
