/**
 * The archive round trip, in a box of its own at the foot of the domain page.
 *
 * Both halves are admin-only endpoints, so the whole card is drawn under
 * `canAdminister` by the screen that mounts it - nothing here is offered to
 * somebody who would be refused at it. They sit together because they are one
 * round trip read from either end: a copy of everything in this domain, and a
 * copy written back into one.
 */

import type { ReactElement } from "react";
import { useId } from "react";

import { archiveDownloadUrl } from "../api/admin";
import { BUTTON } from "./primitives";

export function BackupCard({
  domain,
  onImport,
}: {
  domain: string;
  /** Open the import dialog, which the screen owns because its palette row opens it too. */
  onImport: () => void;
}): ReactElement {
  const headingId = useId();
  return (
    <section
      aria-labelledby={headingId}
      className="flex flex-col gap-3 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <h2 id={headingId} className="text-section">
        Backup
      </h2>
      <p className="text-sm text-slate-600 dark:text-slate-400">
        A zip of every engram in this domain, exactly as they are on disk. The
        same zip is what an import reads, so a domain can be carried to another
        instance or restored into this one.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        {/*
          An anchor rather than a button that fetches: the archive route is a
          cookie-authenticated GET, so the browser saves the file itself and
          this app never holds a whole domain in memory to hand it back.
          `download` is what makes it a save rather than a navigation into a
          zip.
        */}
        <a
          href={archiveDownloadUrl(domain)}
          download
          className={`inline-flex items-center ${BUTTON.secondary}`}
        >
          Download archive
        </a>
        <button type="button" onClick={onImport} className={BUTTON.secondary}>
          Import archive
        </button>
      </div>
    </section>
  );
}
