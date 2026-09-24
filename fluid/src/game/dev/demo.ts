/**
 * The look demo's engine room: everything that is not React.
 *
 * `startDemo` builds the canned bridge, makes a context, wires input,
 * movement, light specials and the renderer to the 35 Hz loop, and returns
 * the one function that undoes all of it. That function is the whole
 * contract with the React shell: StrictMode mounts, unmounts and mounts the
 * shell again in development, which is the only mode this demo exists in, so
 * a cleanup that missed a listener or a frame request would leave two
 * stations running on one canvas.
 *
 * Keys: 1, 2 and 4 pick the look, R toggles the retired condition (the
 * canned bridge is `stable`; retired shows it `archived`, the derelict end
 * of the scale), WASD walks, the arrows turn, the mouse looks once the
 * canvas is clicked.
 */

import { createInput } from "../core/input";
import { createLoop } from "../core/loop";
import { backbufferSize } from "../device";
import { createContext } from "../gl/context";
import { createLights, type LightState } from "../render/lights";
import { LOOKS, lookForKey, type LookId } from "../render/looks";
import { createRenderer, type Renderer } from "../render/renderer";
import { CANNED_BRIDGE } from "../world/canned";
import { generateRoom } from "../world/generate";
import {
  EYE_HEIGHT,
  blockersFor,
  headBob,
  spawnPlayer,
  stepPlayer,
  type Player,
} from "../world/move";
import type { RoomSpec } from "../world/types";

/**
 * Where the demo writes its heads-up text: the look, condition and mouse
 * hint (`status`), the frame time and the colour format of the render
 * targets (`frame`), and a centred notice over the canvas for a missing or
 * lost GPU (`notice`, null hides it). The React shell writes these straight
 * into the DOM, so calling them every quarter second costs no render.
 */
export interface DemoHud {
  status(text: string): void;
  frame(text: string): void;
  notice(text: string | null): void;
}

/** The status R switches to. */
const RETIRED_STATUS = "archived";

/**
 * Starts the look demo on `canvas` and returns its cleanup.
 *
 * `options.forceRgba8` skips the half-float probe, so the RGBA8 bloom path
 * Safari takes can be judged on any browser. When the canvas gives no
 * context the notice says so and nothing runs; when the GPU drops the
 * context the loop stops, and it starts again on a fresh renderer when the
 * browser restores it. The cleanup stops the loop, removes every listener
 * and frees the renderer's GPU objects, but leaves the context itself alone:
 * StrictMode's second mount asks the same canvas for its context and must
 * get a live one back.
 */
export function startDemo(
  canvas: HTMLCanvasElement,
  hud: DemoHud,
  options: { forceRgba8: boolean },
): () => void {
  let lookId: LookId = "day";
  let retired = false;
  let room: RoomSpec = generateRoom(CANNED_BRIDGE);
  let blockers = blockersFor(room);
  let lights: LightState = createLights(room.lights);
  let player: Player = spawnPlayer(room);
  let previous: Player = player;
  let renderer: Renderer | null = null;
  let colorFormat = "";
  let frameSum = 0;
  let frameCount = 0;
  let lastReport = performance.now();
  const started = performance.now();

  const input = createInput(canvas);
  const onClick = () => input.requestLock();
  canvas.addEventListener("click", onClick);

  const showStatus = () => {
    hud.status(
      `${LOOKS[lookId].name.toUpperCase()}  |  ${room.condition.toUpperCase()}  |  ${
        input.locked ? "ESC RELEASES THE MOUSE" : "CLICK TO LOOK AROUND"
      }`,
    );
  };

  const rebuildRoom = () => {
    room = generateRoom({
      ...CANNED_BRIDGE,
      status: retired ? RETIRED_STATUS : CANNED_BRIDGE.status,
    });
    blockers = blockersFor(room);
    lights = createLights(room.lights);
    renderer?.setRoom(room, LOOKS[lookId]);
    showStatus();
  };

  const resize = () => {
    const { width, height } = backbufferSize(
      canvas.clientWidth,
      canvas.clientHeight,
      window.devicePixelRatio,
      1,
    );
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    renderer?.resize(width, height);
  };

  const boot = (): boolean => {
    const context = createContext(canvas, options);
    if (context === null) {
      hud.notice("?DEVICE NOT PRESENT ERROR");
      return false;
    }
    renderer = createRenderer(context.gl, context.caps);
    renderer.setRoom(room, LOOKS[lookId]);
    resize();
    hud.notice(null);
    colorFormat = context.caps.color.toUpperCase();
    hud.frame(colorFormat);
    return true;
  };

  const loop = createLoop({
    tick() {
      for (const code of ["Digit1", "Digit2", "Digit4"]) {
        if (input.pressed(code)) {
          const next = lookForKey(code);
          if (next !== null && next !== lookId) {
            lookId = next;
            renderer?.setRoom(room, LOOKS[lookId]);
            showStatus();
          }
        }
      }
      if (input.pressed("KeyR")) {
        retired = !retired;
        rebuildRoom();
      }
      const axis = (plus: string, minus: string) =>
        (input.held(plus) ? 1 : 0) - (input.held(minus) ? 1 : 0);
      const look = input.takeLook();
      previous = player;
      player = stepPlayer(
        player,
        {
          forward: axis("KeyW", "KeyS"),
          strafe: axis("KeyD", "KeyA"),
          turn: axis("ArrowLeft", "ArrowRight"),
          lookDx: look.dx,
          lookDy: look.dy + axis("ArrowDown", "ArrowUp") * 12,
        },
        room,
        blockers,
      );
      lights.tick();
    },
    render(alpha, frameMs) {
      frameSum += frameMs;
      frameCount++;
      const now = performance.now();
      if (now - lastReport > 250) {
        const ms = frameSum / frameCount;
        hud.frame(
          `${colorFormat}  ${ms.toFixed(1)} MS  ${Math.round(1000 / ms)} FPS`,
        );
        frameSum = 0;
        frameCount = 0;
        lastReport = now;
        showStatus();
      }
      const lerp = (a: number, b: number) => a + (b - a) * alpha;
      renderer?.draw(
        {
          eye: [
            lerp(previous.x, player.x),
            EYE_HEIGHT + headBob(player),
            lerp(previous.z, player.z),
          ],
          yaw: lerp(previous.yaw, player.yaw),
          pitch: lerp(previous.pitch, player.pitch),
        },
        lights.levels,
        (now - started) / 1000,
      );
    },
  });

  const observer = new ResizeObserver(resize);
  observer.observe(canvas);
  const onLost = (e: Event) => {
    e.preventDefault();
    loop.stop();
    renderer = null;
    hud.notice("SIGNAL LOST - WAITING FOR THE GPU");
  };
  const onRestored = () => {
    if (boot()) loop.start();
  };
  canvas.addEventListener("webglcontextlost", onLost);
  canvas.addEventListener("webglcontextrestored", onRestored);

  if (boot()) loop.start();
  showStatus();

  return () => {
    loop.stop();
    observer.disconnect();
    canvas.removeEventListener("click", onClick);
    canvas.removeEventListener("webglcontextlost", onLost);
    canvas.removeEventListener("webglcontextrestored", onRestored);
    input.dispose();
    renderer?.dispose();
    renderer = null;
  };
}
