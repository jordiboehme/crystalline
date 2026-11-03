/**
 * The two engram pages whose tables carry no outer pipes on some or all of
 * their rows (ruling R23): `pipeless.md`, where no row has them, and
 * `mixed.md`, where the header and one data row have them and the other data
 * row has none. Each grid feature has a test that runs on both.
 *
 * Both tables have two columns and two data rows, so a test can name a cell
 * by row and column without knowing the spelling; `cells` is what the grid
 * shows in each one.
 */

import pipeless from "../editor/fixtures/tables/pipeless.md?raw";
import mixed from "../editor/fixtures/tables/mixed.md?raw";

export interface PipelessFixture {
  name: string;
  text: string;
  /** The grid's cell texts, header first. */
  cells: string[][];
  /** Per column, as the delimiter row reads. */
  aligns: string[];
  /** The table's lines, header first, as the page spells them. */
  lines: string[];
}

export const PIPELESS_FIXTURES: PipelessFixture[] = [
  {
    name: "pipeless.md",
    text: pipeless,
    cells: [
      ["a", "b"],
      ["1", "2"],
      ["3", "4"],
    ],
    aligns: ["", ""],
    lines: ["a | b", "--|--", "1 | 2", "3 | 4"],
  },
  {
    name: "mixed.md",
    text: mixed,
    cells: [
      ["a", "b"],
      ["1", "2"],
      ["3", "4"],
    ],
    aligns: ["left", "right"],
    lines: ["| a | b |", "|:--|--:|", "1 | 2", "| 3 | 4 |"],
  },
];

/** The document offset of the table's first character. */
export function tableStart(fixture: PipelessFixture): number {
  const at = fixture.text.indexOf(`\n${fixture.lines[0] ?? ""}\n`);
  if (at < 0) throw new Error(`no table in ${fixture.name}`);
  return at + 1;
}

/** The fixture's page with its table lines replaced by `lines`. */
export function withTable(fixture: PipelessFixture, lines: string[]): string {
  return fixture.text.replace(
    `${fixture.lines.join("\n")}\n`,
    `${lines.join("\n")}\n`,
  );
}

/** A table that stays text: `- | -` is a list item, not a delimiter row. */
export const LIST_ITEM_NOT_A_TABLE =
  "Before.\n\na | b\n- | -\n1 | 2\n\nAfter.\n";
