/**
 * The look demo's engine room: the session on the canned bridge.
 *
 * `startDemo` starts a session with no query client, shows the canned
 * bridge and returns the one function that undoes all of it. That function
 * is the whole contract with the React shell: StrictMode mounts, unmounts
 * and mounts the shell again in development, which is the only mode this
 * demo exists in, so a cleanup that missed a listener or a frame request
 * would leave two stations running on one canvas.
 *
 * Everything the game does in a room works here: the doors open, the
 * terminals open the CRT reader, the keys are the game's. Only travel goes
 * nowhere, since there is no client to load a place with: walking through a
 * door says `SIGNAL LOST` and leaves the player on the bridge.
 *
 * Keys: 1, 2 and 4 pick the look, R toggles the retired condition (the
 * canned bridge is `stable`; retired shows it `archived`, the derelict end
 * of the scale), WASD walks, the arrows turn, the mouse looks once the
 * canvas is clicked, E uses what the player faces and I inverts the
 * vertical look.
 */

import { createSession, type HudSink, type Session } from "../session";
import { CANNED_BRIDGE } from "../world/canned";

/** The status R switches to. */
const RETIRED_STATUS = "archived";

/**
 * Starts the look demo on `canvas` and returns its cleanup and the session,
 * which the shell needs to close the CRT reader.
 *
 * `options.forceRgba8` skips the half-float probe, so the RGBA8 bloom path
 * Safari takes can be judged on any browser. `options.openFluid` is where F
 * sends the engram's Fluid page. R shows the same bridge again with its
 * status swapped, which keeps the player where they stand.
 */
export function startDemo(
  canvas: HTMLCanvasElement,
  hud: HudSink,
  options: { forceRgba8: boolean; openFluid: (path: string) => void },
): { session: Session; stop: () => void } {
  let retired = false;
  const session = createSession({
    canvas,
    client: null,
    hud,
    navigate: () => {},
    openFluid: options.openFluid,
    forceRgba8: options.forceRgba8,
  });
  session.showCanned(CANNED_BRIDGE);

  const onKey = (event: KeyboardEvent) => {
    if (event.code !== "KeyR" || event.repeat) return;
    retired = !retired;
    session.showCanned({
      ...CANNED_BRIDGE,
      status: retired ? RETIRED_STATUS : CANNED_BRIDGE.status,
    });
  };
  window.addEventListener("keydown", onKey);

  return {
    session,
    stop: () => {
      window.removeEventListener("keydown", onKey);
      session.dispose();
    },
  };
}
