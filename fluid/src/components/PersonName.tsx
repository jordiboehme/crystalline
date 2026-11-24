import type { ReactElement } from "react";

import { personLabel, personLabelWithLogin } from "../people";

/**
 * A person's name as text: the display name, with the login name in a tooltip
 * so whoever reads it can always find the account it stands for.
 *
 * `withLogin` shows "Display (login)" for a list that must tell people apart.
 * The text is the accessible name; the tooltip only adds the login.
 */
export function PersonName({
  login,
  display,
  withLogin = false,
  className,
}: {
  login: string;
  display?: string | null | undefined;
  withLogin?: boolean;
  className?: string | undefined;
}): ReactElement {
  return (
    <span title={login} className={className}>
      {withLogin
        ? personLabelWithLogin(login, display)
        : personLabel(login, display)}
    </span>
  );
}
