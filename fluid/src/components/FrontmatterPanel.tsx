/**
 * What an engram declares about itself, beside the engram.
 *
 * Every row here is a field the engram carries. A field it does not carry has
 * no row, which matters most for the temporal ones: in this knowledge base an
 * absent `valid_from` means the knowledge has always been valid and an absent
 * `valid_to` means it is valid forever, so the honest rendering of both is
 * nothing at all. A row reading "valid until: forever", or worse a sentinel
 * date, would be a fact this panel made up, and an agent reading the screen
 * would carry it away as one.
 *
 * Tags are links into search rather than into this domain, because a tag is a
 * thread through the whole knowledge base.
 */

import { Link } from "react-router";

import type { EngramFrontmatter, VerifiedEntry } from "../api/engram";
import { formatDay } from "../format";
import { tagRoute } from "../paths";

export interface FrontmatterPanelProps {
  frontmatter: EngramFrontmatter;
}

export function FrontmatterPanel({ frontmatter }: FrontmatterPanelProps) {
  const { type, status, tags, salience, verified } = frontmatter;
  const validity = validityOf(frontmatter);
  const stamp = latestVerification(verified);

  return (
    <section
      aria-label="Frontmatter"
      className="rounded border border-slate-200 px-4 py-3 dark:border-slate-800"
    >
      <h2 className="mb-2 text-sm font-semibold">Frontmatter</h2>
      <dl className="flex flex-col gap-2 text-sm">
        {type !== null && (
          <Row label="Type">
            <span>{type}</span>
          </Row>
        )}
        {status !== null && (
          <Row label="Status">
            <span>{status}</span>
          </Row>
        )}
        {tags.length > 0 && (
          <Row label="Tags">
            <span className="flex flex-wrap gap-x-2 gap-y-1">
              {tags.map((tag) => (
                <Link
                  key={tag}
                  to={tagRoute(tag)}
                  className="text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
                >
                  #{tag}
                </Link>
              ))}
            </span>
          </Row>
        )}
        {salience !== null && (
          <Row label="Salience">
            <span className="tabular-nums">{salience}</span>
          </Row>
        )}
        {validity !== null && (
          <Row label="Valid">
            <span className="tabular-nums">{validity}</span>
          </Row>
        )}
        {stamp !== null && (
          <Row label="Verified">
            <span>{stamp}</span>
          </Row>
        )}
      </dl>
    </section>
  );
}

/** One field, drawn only by a caller that has one. */
function Row({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="text-xs text-slate-500 dark:text-slate-400">{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

/**
 * The validity window as a line, or null when the engram bounds it at neither
 * end. Each end is stated only where the engram states it: an open end reads as
 * an open end rather than as a date.
 */
function validityOf(frontmatter: EngramFrontmatter): string | null {
  const { validFrom, validTo } = frontmatter;
  if (validFrom !== null && validTo !== null) {
    return `${validFrom} to ${validTo}`;
  }
  if (validFrom !== null) {
    return `from ${validFrom}`;
  }
  if (validTo !== null) {
    return `until ${validTo}`;
  }
  return null;
}

/**
 * The most recent verification, as a stamp.
 *
 * The trail is a list and the last entry is the one that still speaks for the
 * engram. A legacy `last_verified` records no actor, so that stamp is a date
 * alone rather than a date attributed to nobody.
 */
function latestVerification(entries: VerifiedEntry[]): string | null {
  const latest = entries[entries.length - 1];
  if (!latest) {
    return null;
  }
  const day = latest.at === null ? null : formatDay(latest.at);
  if (latest.by === null) {
    return day;
  }
  return day === null ? latest.by : `${latest.by} on ${day}`;
}
