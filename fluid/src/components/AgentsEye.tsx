/**
 * The same engram, read the way an agent reads it.
 *
 * Three facts, and none of them are on the page anywhere else in this shape.
 * The domain's routing lines are what sends an agent here in the first place,
 * and they live in the MANIFEST rather than in the engram, so a reader editing
 * knowledge nobody retrieves has no other way to see why. The salience is what
 * ranks this engram once it is in the running. The token cost is what reading
 * it spends out of a budget the reader never sees.
 *
 * The cost is an estimate and says so. Characters over four is the rule of
 * thumb, not a count from a tokenizer, and a number presented as measured would
 * be a number somebody plans against.
 *
 * Everything here is what the engram and its domain actually carry. A salience
 * nobody wrote has no row, in the same way an absent `valid_to` has none on the
 * frontmatter panel: the honest rendering of a fact nobody recorded is nothing
 * at all.
 *
 * Folded away by default, like the neighborhood below it. This is a second way
 * of reading a page whose first job is the prose.
 */

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";

/** How many characters one token is worth, as a rule of thumb. */
const CHARS_PER_TOKEN = 4;

export interface AgentsEyeProps {
  /** The domain this engram lives in, whose routing lines are read here. */
  domain: string;
  /** The engram's `salience`, or null when it carries none. */
  salience: number | null;
  /** The markdown as written, which is what an agent is handed. */
  content: string;
}

export function AgentsEye({ domain, salience, content }: AgentsEyeProps) {
  const [open, setOpen] = useState(false);
  // The listing the sidebar already read, under the same key: the routing
  // lines cost nothing on the wire.
  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  const routing =
    listing.data?.domains.find((entry) => entry.name === domain)?.whenToUse ??
    [];
  const tokens = Math.ceil(content.length / CHARS_PER_TOKEN);

  return (
    <section aria-labelledby="engram-agents-eye">
      <h2 id="engram-agents-eye" className="mb-2 text-lg font-semibold">
        Agent&apos;s eye
      </h2>
      <button
        type="button"
        aria-expanded={open}
        aria-controls="engram-agents-eye-panel"
        onClick={() => {
          setOpen((was) => !was);
        }}
        className="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
      >
        {open ? "Hide what an agent is taught" : "Show what an agent is taught"}
      </button>
      <div id="engram-agents-eye-panel" className="mt-3">
        {open && (
          <div className="flex flex-col gap-3 rounded border border-slate-200 px-4 py-3 text-sm dark:border-slate-800">
            <p className="text-slate-500 dark:text-slate-400">
              This is what an agent is taught about this knowledge.
            </p>
            <dl className="flex flex-col gap-3">
              {routing.length > 0 && (
                <div className="flex flex-col gap-1">
                  <dt className="text-xs text-slate-500 dark:text-slate-400">
                    Routing, from the {domain} MANIFEST
                  </dt>
                  <dd>
                    <ul className="list-disc pl-5">
                      {routing.map((line) => (
                        <li key={line}>{line}</li>
                      ))}
                    </ul>
                  </dd>
                </div>
              )}
              {salience !== null && (
                <div className="flex flex-col gap-1">
                  <dt className="text-xs text-slate-500 dark:text-slate-400">
                    Salience
                  </dt>
                  <dd className="tabular-nums">{salience}</dd>
                </div>
              )}
              <div className="flex flex-col gap-1">
                <dt className="text-xs text-slate-500 dark:text-slate-400">
                  Context cost
                </dt>
                <dd>
                  <span className="tabular-nums">About {tokens} tokens</span>
                  <span className="text-slate-500 dark:text-slate-400">
                    {" "}
                    (approximate: the characters of this engram over four, not a
                    count from a tokenizer)
                  </span>
                </dd>
              </div>
            </dl>
          </div>
        )}
      </div>
    </section>
  );
}
