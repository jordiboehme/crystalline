/**
 * The two things a dialog says about the identity a write would go out on, in
 * one place because two dialogs say them: the share dialog and the withdraw
 * dialog are the same decision about the same credential, and a wording that
 * drifted between them would read as two different rules.
 *
 * {@link useShareIdentity} is what decides which of them is drawn.
 */

import type { ReactElement } from "react";
import { Link } from "react-router";

import { profileRoute } from "../paths";
import { BUTTON } from "./primitives";

/**
 * The primary action for a session that has no identity to write with: the fix,
 * where the write would have been.
 *
 * A link rather than a button, and to the profile rather than into a flow
 * started here: connecting is a device code typed in another window or a token
 * pasted, both of which live on the profile card and neither of which belongs
 * inside a dialog about a proposal.
 */
export function ConnectToShare(): ReactElement {
  return (
    <Link to={profileRoute()} className={`inline-block ${BUTTON.primary}`}>
      Connect GitHub to share
    </Link>
  );
}

/**
 * Whose name the write would carry, said quietly beside the button that makes
 * it.
 *
 * `mr-auto` rather than a row of its own: this belongs to the action, and a
 * line above the buttons would read as another thing to decide about.
 */
export function SharingAs({ login }: { login: string }): ReactElement {
  return (
    <span className="mr-auto text-caption text-slate-500 dark:text-slate-400">
      {`Sharing as @${login}`}
    </span>
  );
}
