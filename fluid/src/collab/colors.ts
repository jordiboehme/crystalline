/**
 * Who is who in a room, in one color each.
 *
 * The color is derived from the account name rather than handed out by the
 * server: every participant computes the same color for the same person
 * without a round trip, and a reconnect never repaints the room.
 */

/** The palette y-codemirror.next paints cursors and selections with; the
 *  light variant is the selection background (the upstream 0x33 alpha
 *  convention). Chosen for contrast on both schemes. */
const PALETTE = [
  "#0ea5e9",
  "#f59e0b",
  "#10b981",
  "#8b5cf6",
  "#ef4444",
  "#14b8a6",
  "#f97316",
  "#ec4899",
] as const;

/** A deterministic presence color per account name, from a fixed palette. */
export function presenceColor(name: string): {
  color: string;
  colorLight: string;
} {
  let hash = 0;
  for (const unit of name) {
    hash = (hash * 31 + unit.charCodeAt(0)) >>> 0;
  }
  const color = PALETTE[hash % PALETTE.length] ?? PALETTE[0];
  return { color, colorLight: `${color}33` };
}
