/**
 * The game route: every place of the station under `/π` (M3 C2, C4), read
 * from the location itself with `addressOfGameLocation` - the airlock at
 * `/π`, a domain's bridge at `/π/d/<domain>`, a deck at the same with
 * `?path=<folder>` (and `&section=<n>`), an engram's room at
 * `/π/d/<domain>/e/<permalink>`, and the airlock again for any other path
 * under the prefix - and every room the player walks on to from there.
 *
 * Development only until milestone 4 gives it a way in: the route that
 * renders this exists only under `import.meta.env.DEV` (see `routes.tsx`),
 * so a production build never contains it.
 *
 * The URL and the session take turns. The URL is where the player means to
 * be: the session is started on it, and a URL that changes under a running
 * session (a manual edit, the browser's back and forward) sends the player
 * there with `go`. The session is where the player is: when it lands in a
 * room it replaces the URL with that room's route, so a reload comes back
 * to the same room and the history is not filled with every door walked
 * through. That replace changes the location too, and the route must not
 * answer it with a second journey to the room the player just entered,
 * which would reload the room and lose where the door put them. So the
 * route keeps the one station address the URL is following: the one it
 * last sent the session to, or, once the session lands somewhere of its
 * own accord, the address it entered, set before the session's replace
 * reaches the router. A new URL is followed exactly when it names another
 * address (`sameStation`: kind, domain, folder and section, or
 * permalink). It is not
 * compared with the room the player stands in: going back to that room
 * while another one is still loading must cancel the load, and going
 * forward again to a room that failed to load must try it again.
 *
 * The device is refused before the first paint exactly as in the look
 * demo, and the session is created in an effect once the canvas exists and
 * disposed in its cleanup. The HUD is written by the session straight into
 * the DOM (`useHud`); the CRT reader is mounted while a terminal is read,
 * and closing it hands the keys back to the session.
 *
 * Typing `idclev` opens the level select over the station (`LevelSelect`):
 * the session says so through `onLevels`, which only this route passes, so
 * the look demo and the model gallery ignore the word. The select lists
 * the domains, marks the one the URL names (none in the airlock), and on
 * Enter or a click asks the
 * session to `jump` to that domain's bridge, a journey like any other that
 * replaces the URL when it lands. Esc hands the keys back through
 * `closeLevels`.
 *
 * Only this route passes the session its `consoleRoom` option, so only
 * here does walking through a police box's open doors lead into the
 * console room, and its inner doors out to a domain's bridge picked from
 * the domain listing (`loadDomainRows`, the sidebar's own cached query).
 * The console room has no address: the URL keeps naming the room walked
 * in from, so a reload inside comes back there.
 *
 * F, in the room and in the CRT reader alike, opens the Fluid page of the
 * address the player stands at (`fluidRouteOfStation`): `/` in the
 * airlock, the domain page on a bridge, the folder on a deck, the engram's
 * reading page in its room.
 */

import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";

import { loadDomainRows } from "./data/source";
import { detectEnvironment, refusalReason, type Refusal } from "./device";
import { hasWebGL2 } from "./gl/context";
import {
  addressOfGameLocation,
  domainOf,
  fluidRouteOfStation,
  sameStation,
} from "./paths";
import { createSession, type Session } from "./session";
import { DeviceRefusal } from "./ui/DeviceRefusal";
import { GAME_LEGEND } from "./ui/keys";
import { LevelSelect } from "./ui/LevelSelect";
import { StationView } from "./ui/StationView";
import { useHud } from "./ui/useHud";
import type { StationAddress } from "./world/types";

/** Opens a Fluid page in a new tab: the F key, in the room and the reader. */
function openFluid(path: string) {
  window.open(path, "_blank", "noopener");
}

/**
 * The station address a location names. The route only matches under the
 * `π` prefix, so `addressOfGameLocation` answers an address there; the
 * airlock stands in for the null it keeps for a path outside the prefix.
 */
function addressAt(pathname: string, search: string): StationAddress {
  return addressOfGameLocation(pathname, search) ?? { kind: "airlock" };
}

/**
 * The route's screen. See the module doc for how the URL and the session
 * take turns.
 *
 * `navigate` is held in a ref rather than handed to the session's effect as
 * a dependency: the router may make a new one whenever the location
 * changes, and a session torn down and rebuilt for that would lose its room
 * on every door. The address is held in a ref for the same reason: the
 * session is created once, on the address the URL names at that moment,
 * and every later address reaches it through `go`.
 */
export default function GameRoute() {
  const { pathname, search } = useLocation();
  const address = addressAt(pathname, search);
  const client = useQueryClient();
  const navigate = useNavigate();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sessionRef = useRef<Session | null>(null);
  const navigateRef = useRef(navigate);
  const addressRef = useRef<StationAddress>(address);
  // The address the URL is following: the one this route last sent the
  // session to, or the one the session last landed in of its own accord.
  // The location effect follows a URL that names any other address.
  const requestedRef = useRef<StationAddress | null>(null);
  const { sink, view, connector, reader } = useHud();
  const [refusal] = useState<Refusal | null>(() =>
    refusalReason(detectEnvironment(hasWebGL2)),
  );
  const [levels, setLevels] = useState(false);

  useEffect(() => {
    navigateRef.current = navigate;
  }, [navigate]);

  useEffect(() => {
    addressRef.current = addressAt(pathname, search);
  }, [pathname, search]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (refusal !== null || canvas === null) return;
    const session = createSession({
      canvas,
      client,
      hud: sink,
      navigate: (path) => {
        // The session landed somewhere of its own accord: that is now the
        // address the URL is following, not the one this route asked for.
        requestedRef.current = sessionRef.current?.current ?? null;
        void navigateRef.current(path, { replace: true });
      },
      openFluid,
      forceRgba8: false,
      onLevels: setLevels,
      consoleRoom: { domains: (signal) => loadDomainRows(client, signal) },
    });
    sessionRef.current = session;
    const first = addressRef.current;
    requestedRef.current = first;
    session.go(first);
    return () => {
      sessionRef.current = null;
      requestedRef.current = null;
      session.dispose();
    };
  }, [refusal, client, sink]);

  useEffect(() => {
    const session = sessionRef.current;
    if (session === null) return;
    const next = addressAt(pathname, search);
    if (sameStation(requestedRef.current, next)) return;
    requestedRef.current = next;
    session.go(next);
  }, [pathname, search]);

  const closeReader = useCallback(() => {
    sessionRef.current?.closeReader();
  }, []);
  const readerOpenFluid = useCallback(() => {
    const current = sessionRef.current?.current;
    if (current) openFluid(fluidRouteOfStation(current));
  }, []);
  const closeLevels = useCallback(() => {
    sessionRef.current?.closeLevels();
  }, []);
  // The jump is the session's own journey (C10): it replaces the URL once
  // the bridge lands, and the navigate callback above moves
  // `requestedRef` with it, so the params effect does not travel twice.
  const jump = useCallback((name: string) => {
    sessionRef.current?.jump(name);
  }, []);

  if (refusal !== null) return <DeviceRefusal />;
  return (
    <>
      <StationView
        canvasRef={canvasRef}
        view={view}
        connector={connector}
        reader={reader}
        onCloseReader={closeReader}
        onOpenFluid={readerOpenFluid}
        legend={GAME_LEGEND}
      />
      {levels && (
        <LevelSelect
          current={domainOf(address) ?? ""}
          onJump={jump}
          onClose={closeLevels}
        />
      )}
    </>
  );
}
