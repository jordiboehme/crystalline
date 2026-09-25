/**
 * The look demo's engine room: the session on a canned place.
 *
 * `startDemo` starts a session with no query client, shows `options.place`
 * (the canned bridge unless the caller names another) and returns the one
 * function that undoes all of it. That function is the whole contract with
 * the React shell: StrictMode mounts, unmounts and mounts the shell again
 * in development, which is the only mode this demo exists in, so a cleanup
 * that missed a listener or a frame request would leave two stations
 * running on one canvas.
 *
 * Everything the game does in a room works here: the doors open, the
 * terminals open the CRT reader, the keys are the game's. Only travel goes
 * nowhere, since there is no client to load a place with: walking through a
 * door says `SIGNAL LOST` and leaves the player on the place shown.
 *
 * Keys: 1, 2 and 4 pick the look, R toggles the retired condition (the
 * place's own status; retired shows `archived`, the derelict end of the
 * scale), WASD walks, the arrows turn, the mouse looks once the canvas is
 * clicked, E uses what the player faces and I inverts the vertical look.
 *
 * `options.props` false shows the place undressed: `session.showRoom` with
 * `generateRoom`'s props and heroes stripped, rather than `session.showCanned`, which
 * is the dev-only comparison `?props=0` reads. That path has no client-side
 * `PlaceInput` kept by the session, so its terminals open no reader; R
 * still swaps the condition, rebuilding the same way.
 */

import { createSession, type HudSink, type Session } from "../session";
import { CANNED_BRIDGE } from "../world/canned";
import { generateRoom } from "../world/generate";
import type { PlaceInput } from "../world/types";

/** The status R switches to. */
const RETIRED_STATUS = "archived";

/**
 * Starts the look demo on `canvas` and returns its cleanup and the session,
 * which the shell needs to close the CRT reader.
 *
 * `options.forceRgba8` skips the half-float probe, so the RGBA8 bloom path
 * Safari takes can be judged on any browser. `options.openFluid` is where F
 * sends the engram's Fluid page. `options.place` is the canned bridge
 * unless the caller names another, and `options.props` is true unless the
 * caller asks for the undressed comparison (no props and no heroes). R shows the same place again
 * with its status swapped, which keeps the player where they stand.
 */
export function startDemo(
  canvas: HTMLCanvasElement,
  hud: HudSink,
  options: {
    forceRgba8: boolean;
    openFluid: (path: string) => void;
    place?: PlaceInput;
    props?: boolean;
  },
): { session: Session; stop: () => void } {
  const place = options.place ?? CANNED_BRIDGE;
  const withProps = options.props ?? true;
  let retired = false;
  const session = createSession({
    canvas,
    client: null,
    hud,
    navigate: () => {},
    openFluid: options.openFluid,
    forceRgba8: options.forceRgba8,
  });
  const show = (p: PlaceInput) => {
    if (withProps) session.showCanned(p);
    else session.showRoom({ ...generateRoom(p), props: [], heroes: [] });
  };
  show(place);

  const onKey = (event: KeyboardEvent) => {
    if (event.code !== "KeyR" || event.repeat) return;
    retired = !retired;
    show({
      ...place,
      status: retired ? RETIRED_STATUS : place.status,
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
