/**
 * The C64's answer to a device that cannot run the station.
 *
 * Every screen that hosts a session (the game route, the look demo and the
 * model gallery) decides once, before its first paint, whether the device
 * is refused (`refusalReason` in `device.ts`), and shows this instead of a
 * canvas when it is: the error in light blue on the dark blue screen, and
 * RUN/STOP back to the app.
 *
 * The text is the light blue lightened (`C64_TEXT`): the machine's own
 * light blue on its blue reads at 2.3:1, under WCAG AA's 4.5:1 for normal
 * text, so the border keeps the palette's colour and the text is raised
 * to about 4.7:1 in the same hue.
 */

import { Link } from "react-router";

/** The C64's screen colour, which the pause screen shares. */
export const C64_BLUE = "#352879";
/** The C64's border colour, which the pause screen shares. */
export const C64_LIGHT_BLUE = "#6c5eb5";
/**
 * The text on `C64_BLUE`: the light blue lightened until it reaches WCAG
 * AA (4.5:1) on the blue, which the pause and connecting screens share.
 */
export const C64_TEXT = "#a49bd1";

/** The refusal screen, full screen and with one way out. */
export function DeviceRefusal() {
  return (
    <div
      className="fixed inset-0 flex items-center justify-center p-8 font-mono text-lg uppercase"
      style={{ background: C64_LIGHT_BLUE }}
    >
      <div
        className="w-full max-w-2xl p-8"
        style={{ background: C64_BLUE, color: C64_TEXT }}
      >
        <p>?DEVICE NOT PRESENT ERROR</p>
        <p>READY.</p>
        <p className="mt-6">
          <Link to="/" className="underline">
            RUN/STOP
          </Link>
        </p>
      </div>
    </div>
  );
}
