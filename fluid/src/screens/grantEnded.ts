import type { OauthGrantEndReason } from "../api/model";

/**
 * Why a connected client is gone, in one plain sentence for the person who
 * finds it missing. The server keeps this record for seven days after the
 * grant ended, so the line shows for that long and then goes by itself; there
 * is nothing to dismiss.
 */
export function grantEndedText(reason: OauthGrantEndReason): string {
  switch (reason) {
    case "replayed":
      return "Signed out because an old sign-in token was used again.";
    case "revoked":
      return "You revoked it.";
    case "client_revoked":
      return "The client signed out.";
    case "password_reset":
      return "Signed out because the password was reset.";
  }
}
