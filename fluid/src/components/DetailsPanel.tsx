/**
 * The quiet metadata column beside an engram: small muted labels over chip
 * values, hairline dividers, no box around any of it. The prose is what the
 * page is for, so this column states the engram's own fields and gets out of
 * the way.
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
 * thread through the whole knowledge base. The address is here rather than in
 * the page header: `crystalline://domain/permalink` is what this engram is
 * called everywhere else, which makes it a fact about the engram like the
 * others rather than a control.
 */

import { Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { Link } from "react-router";

import type { EngramFrontmatter, VerifiedEntry } from "../api/engram";
import { formatActor, formatDay } from "../format";
import { tagRoute } from "../paths";
import { Chip, FOCUS_RING, IconButton, statusVariant } from "./primitives";

/** How long the copy outcome stays announced. */
const COPIED_FOR_MS = 2000;

export interface DetailsPanelProps {
  frontmatter: EngramFrontmatter;
  /** The engram's `crystalline://` address, which is what it is called. */
  address: string;
}

export function DetailsPanel({ frontmatter, address }: DetailsPanelProps) {
  const { type, status, tags, salience, verified, generatedBy } = frontmatter;
  const validity = validityOf(frontmatter);
  const stamp = latestVerification(verified);

  return (
    <section aria-label="Details" className="text-sm">
      <h2 className="mb-3 text-caption font-semibold text-slate-500 dark:text-slate-400">
        Details
      </h2>
      <dl className="flex flex-col divide-y divide-slate-100 dark:divide-slate-800">
        {status !== null && (
          <Row label="Status">
            <Chip variant={statusVariant(status)}>{status}</Chip>
          </Row>
        )}
        {type !== null && (
          <Row label="Type">
            <Chip>{type}</Chip>
          </Row>
        )}
        {tags.length > 0 && (
          <Row label="Tags">
            <span className="flex flex-wrap gap-1">
              {tags.map((tag) => (
                <Link
                  key={tag}
                  to={tagRoute(tag)}
                  className={`rounded ${FOCUS_RING}`}
                >
                  <Chip variant="accent">#{tag}</Chip>
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
        {generatedBy !== null && (
          <Row label="Captured by">
            <span>{formatActor(generatedBy)}</span>
          </Row>
        )}
        {stamp !== null && (
          <Row label="Verified">
            <span>{stamp}</span>
          </Row>
        )}
        <Row label="Address">
          <span className="flex min-w-0 items-center gap-1">
            <code className="truncate font-mono text-caption" title={address}>
              {address}
            </code>
            <CopyAddress address={address} />
          </span>
        </Row>
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
    <div className="flex flex-col gap-1 py-2">
      <dt className="text-caption text-slate-500 dark:text-slate-400">
        {label}
      </dt>
      <dd className="min-w-0">{children}</dd>
    </div>
  );
}

/**
 * Hand the engram's address to the clipboard.
 *
 * The outcome is announced in a live region beside the button rather than
 * written into the button's own label. A control that renames itself is a
 * control a reader navigating by name loses track of, and a label that changes
 * silently is no announcement at all: the region is in the document from the
 * start and empty, so the text arriving in it is what gets read out.
 */
function CopyAddress({ address }: { address: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");

  useEffect(() => {
    if (state !== "copied") {
      return;
    }
    const timer = setTimeout(() => {
      setState("idle");
    }, COPIED_FOR_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [state]);

  return (
    <span className="inline-flex items-center gap-1">
      <IconButton
        label="Copy address"
        icon={Copy}
        onClick={() => {
          void (async () => {
            try {
              await navigator.clipboard.writeText(address);
              setState("copied");
            } catch {
              // A browser that refuses the clipboard is not a failure of the
              // page: the address is written out right beside the button
              // either way, and saying so beats a control that silently does
              // nothing.
              setState("failed");
            }
          })();
        }}
      />
      <span
        role="status"
        aria-live="polite"
        aria-label="Copy address result"
        className="text-caption text-slate-500 dark:text-slate-400"
      >
        {state === "copied"
          ? "Copied"
          : state === "failed"
            ? "Copy refused"
            : ""}
      </span>
    </span>
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
