/**
 * MODEL GALLERY: every model the station draws, in one room, for judging.
 *
 * The room is `galleryRoom()`, built by hand rather than generated from an
 * engram, and shown with `session.showRoom`: a door of every style, the
 * sealed ways, the portals, a terminal, a hatch, a poster and the placard in
 * the hall with every kind of furniture, the twelve machines in bays 1 and
 * 2, one of every prop kind and variant in bays 3 and 4 (the floor kinds
 * spilling from rows 1, 3 and 5 onto row 7 once the four tall kinds no
 * longer fit), and a row of every ceiling span kind and variant over row 6
 * of those same two bays.
 * Development only - the route that renders this exists only under
 * `import.meta.env.DEV`, and stays that way for good.
 *
 * The session has no query client, so walking through a door says
 * `SIGNAL LOST` and leaves the player in the gallery, and F opens nothing:
 * the gallery is no engram. `?bloom=rgba8` forces the RGBA8 bloom path as
 * in the look demo.
 *
 * Three more parameters are dev-only and not on the legend. `?hall=heroes`
 * shows `heroHallRoom()` (H15) instead of `galleryRoom()`: one of every hero
 * kind and variant, for judging them and for the `?at=` shots below.
 * `?hall=variants` (2.7 C24) shows `variantsHallRoom()` instead: one of
 * every machine kind and variant along the east wall and its bays, one of
 * every terminal variant on the west wall, and one of every decor kind and
 * variant on the floor, for judging each variant's model against its
 * kind's variant 0 (`?at=machine:<n>`, `?at=terminal:<n>` and
 * `?at=decor:<kind>:<n>`, below).
 * `?at=prop:<kind>:<n>` (H16, see `spotView`) puts the player in front of
 * the n-th hero or prop of that kind in whichever room is shown, framed
 * from its front instead of just facing it. The same pattern reaches
 * curios too (C18, 2.6b), framed close and tilted down, which is why the
 * room is shown through `session.showRoom`'s `view` argument rather than
 * only its `spawn`. An optional view suffix frames any of them from
 * elsewhere (2.6d C20): `:back` from behind, `:side` from the right side,
 * `:quarter` from halfway between the front and the right, and `:close` a hero
 * or prop from nearer (`?at=prop:<kind>:<n>[:back|:side|:quarter|:close]`);
 * `?at=<kind>:<n>` still puts the player in front of the n-th fixture of
 * that kind, facing it, instead of the room's own entrance, to judge a
 * malfunctioning fixture without walking across the hall. `?fault=missing`
 * or `?fault=denied` answers every travel with that failure (the session's
 * loader seam, which answers what `loadStation` would) instead of
 * `SIGNAL LOST`, so every open
 * door, portal and hatch in the gallery malfunctions once the player walks
 * into or crawls through it, and stays broken for the rest of the visit.
 * The sealed sliding door (`door:4`) and the sealed portal (`portal:2`)
 * malfunction from the start, with or without `?fault=`. `?fault=` and
 * `?hall=heroes` are not meant together: the hero hall carries no
 * malfunctioning fixture.
 *
 * `?hall=console` (2.6e C21) shows `consoleRoom()` on its own instead, for
 * judging its fittings without walking in through a police box: no
 * `consoleRoom` option reaches `createSession` here, so the inner doors
 * lead nowhere in the gallery, same as every other door. `?view=<name>`
 * puts the player at one of `CONSOLE_VIEWS` (`entry`, `console`, `rotor`,
 * `wall`, `doors`, `scanner`, `corner`, from `dev/consoleViews.ts`); a
 * missing or unknown name falls back to `entry`. `?at=` and `?fault=` are
 * not meant with `?hall=console`: the console room carries no fixture and
 * no prop or curio `?at=` could frame.
 *
 * `?hall=lifts` (M3 C7, C24, C28) shows `liftsHallRoom()` instead, for
 * judging the station's lift, wall screen and exit: the lift on the
 * entrance edge behind the spawn, listing fourteen domains with a key, the
 * current stop's mark, the overflow line and a note on its panel; the
 * screen across the hall with the key on its heading; the exit two cells
 * east of the lift. `?at=lift:0`, `?at=screen:0` and `?at=exit:0` put the
 * player in front of each. The lift and the exit lead nowhere in the
 * gallery.
 *
 * `?hall=deck` (M3 C8 to C10) shows the deck hub `generateDeck` builds of
 * `CANNED_DECK`, for judging the corridor, its doors, the deck lift at the
 * entrance and the deck's screen at the north end: its first section (24
 * doors), or the one `?section=<n>` names (1-based, as the game's route
 * spells it; `?section=2` is the short section the deck golden pins). The
 * spawn is the deck's own, in front of the lift; `?at=screen:0` stands in
 * front of the screen. The doors and the lift lead nowhere in the gallery.
 *
 * `?hall=bridge` (M3 C20, C21) shows `CANNED_BRIDGE`'s room fitted with
 * `CANNED_BRIDGE_DATA` (`withBridge`), for judging the bridge's own deck
 * lift and wall screen composed with the room the generator already dressed:
 * `?at=lift:0` stands in front of the lift, `?at=screen:0` in front of the
 * screen. Every door, portal and hatch leads nowhere in the gallery, as in
 * `galleryRoom`.
 *
 * `?hall=airlock` (M3 C24) shows the airlock `airlockRoom` builds of
 * `CANNED_DOMAINS`, for judging the round chamber, its light, the lift at
 * the entrance, the outer hatch opposite it with its beacons and the
 * directory board over it (the key on the private domain), the iris light,
 * the hazard ring and the suit lockers. The spawn is the airlock's own, in
 * front of the lift facing the hatch; `?at=lift:0` stands in front of the
 * lift and `?at=screen:0` in front of the hatch under the board. The lift
 * leads nowhere in the gallery.
 *
 * `?hall=hangar` (M3 C13 to C17) shows the hangar `generateDeck` builds of
 * `CANNED_HANGAR`, for judging the hall, its doors on the west, east and
 * south walls, the cargo round the empty landing pads, the pad stencils,
 * the deck's lift and screen, and the structure the room mesh draws from
 * the room's `hangar` data: the bay door, the pads' plates and paint, and
 * the gantries with their legs and catwalks. The spawn is the hangar's own, in
 * front of the lift; `?at=screen:0` stands in front of the screen. The
 * doors and the lift lead nowhere in the gallery.
 *
 * `?pitch=<degrees>` (dev-only, any hall) starts the view tilted up
 * (positive) or down by that much, clamped as the mouse is, for stills of
 * a ceiling or of a board hung high.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { detectEnvironment, refusalReason, type Refusal } from "../device";
import { hasWebGL2 } from "../gl/context";
import { createSession, type PlaceLoader, type Session } from "../session";
import { DeviceRefusal } from "../ui/DeviceRefusal";
import { CLASSIC_KEYS } from "../ui/keys";
import { StationView } from "../ui/StationView";
import { useHud } from "../ui/useHud";
import {
  CANNED_BRIDGE,
  CANNED_BRIDGE_DATA,
  CANNED_DECK,
  CANNED_DOMAINS,
  CANNED_HANGAR,
  galleryRoom,
  heroHallRoom,
  liftsHallRoom,
  variantsHallRoom,
} from "../world/canned";
import { airlockRoom } from "../world/airlock";
import { withBridge } from "../world/bridge";
import { consoleRoom } from "../world/consoleRoom";
import { generateDeck } from "../world/deck";
import { generateRoom } from "../world/generate";
import { CONSOLE_VIEWS, consoleView } from "./consoleViews";
import { spotView } from "./spots";

/** The keys, along the top of the screen. */
const LEGEND = `MODEL GALLERY · ${CLASSIC_KEYS} · WASD MOUSE · I INVERT · 1 DAY SHIFT · 2 APERTURE GRID · 4 FREESCAPE 64`;

/** F goes nowhere from the gallery: there is no engram to open. */
function openNothing() {}

/**
 * The gallery screen. The device is refused before the first paint as in
 * the look demo, and the session is started in an effect once the canvas
 * exists and disposed in its cleanup.
 */
export default function Gallery() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sessionRef = useRef<Session | null>(null);
  const { sink, view, connector, reader } = useHud();
  const [refusal] = useState<Refusal | null>(() =>
    refusalReason(detectEnvironment(hasWebGL2)),
  );

  useEffect(() => {
    const canvas = canvasRef.current;
    if (refusal !== null || canvas === null) return;
    const params = new URLSearchParams(window.location.search);
    const hall = params.get("hall");
    const base =
      hall === "console"
        ? consoleRoom()
        : hall === "heroes"
          ? heroHallRoom()
          : hall === "variants"
            ? variantsHallRoom()
            : hall === "lifts"
              ? liftsHallRoom()
              : hall === "deck"
                ? generateDeck(
                    CANNED_DECK,
                    Math.max(
                      0,
                      Math.floor(Number(params.get("section") ?? "1")) - 1 || 0,
                    ),
                  )
                : hall === "bridge"
                  ? withBridge(
                      CANNED_BRIDGE,
                      generateRoom(CANNED_BRIDGE),
                      CANNED_BRIDGE_DATA,
                    )
                  : hall === "airlock"
                    ? airlockRoom({ domains: CANNED_DOMAINS, here: null })
                    : hall === "hangar"
                      ? generateDeck(CANNED_HANGAR, 0)
                      : galleryRoom();
    const at = params.get("at");
    const roomView =
      hall === "console"
        ? (consoleView(params.get("view")) ?? CONSOLE_VIEWS.entry)
        : at === null
          ? null
          : spotView(base, at);
    const fault = params.get("fault");
    const load: PlaceLoader | undefined =
      fault === "missing" || fault === "denied"
        ? () => Promise.resolve({ kind: fault })
        : undefined;
    const session = createSession({
      canvas,
      client: null,
      hud: sink,
      navigate: openNothing,
      openFluid: openNothing,
      forceRgba8: params.get("bloom") === "rgba8",
      ...(load === undefined ? {} : { load }),
    });
    sessionRef.current = session;
    // `?pitch=<degrees>` tilts the first view up (positive) or down, for a
    // still of a ceiling or a board hung high.
    const tilt = Number(params.get("pitch") ?? "");
    const pitch =
      Number.isFinite(tilt) && params.has("pitch")
        ? (tilt * Math.PI) / 180
        : null;
    session.showRoom(
      roomView === null ? base : { ...base, spawn: roomView.spawn },
      pitch !== null
        ? { pitch }
        : roomView === null
          ? undefined
          : { pitch: roomView.pitch },
    );
    return () => {
      sessionRef.current = null;
      session.dispose();
    };
  }, [refusal, sink]);

  const closeReader = useCallback(() => {
    sessionRef.current?.closeReader();
  }, []);

  if (refusal !== null) return <DeviceRefusal />;
  return (
    <StationView
      canvasRef={canvasRef}
      view={view}
      connector={connector}
      reader={reader}
      legend={LEGEND}
      onCloseReader={closeReader}
      onOpenFluid={openNothing}
    />
  );
}
