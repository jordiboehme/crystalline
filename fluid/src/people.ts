/**
 * How a person is named on screen.
 *
 * The server keeps the login name as the stable identity and sends a
 * person's display name beside it (`display`, or `<field>_display`) when the
 * account has one other than its login. A name with no account (an agent,
 * the machine owner, a removed account) comes without one and is shown as it
 * is. These two helpers are the one place that choice is made, so every
 * screen names a person the same way.
 */

/** The display name when there is one that differs from the login, else the login. */
export function personLabel(login: string, display?: string | null): string {
  const shown = display?.trim();
  return shown && shown !== login ? shown : login;
}

/**
 * "Display (login)" where a list has to tell people apart, or the login alone
 * when there is no other display name.
 */
export function personLabelWithLogin(
  login: string,
  display?: string | null,
): string {
  const shown = personLabel(login, display);
  return shown === login ? login : `${shown} (${login})`;
}

/** The display name a `{ login: display }` map holds for a login, if any. */
export function displayFrom(
  map: Readonly<Record<string, string>> | null | undefined,
  login: string,
): string | null {
  return map?.[login] ?? null;
}
