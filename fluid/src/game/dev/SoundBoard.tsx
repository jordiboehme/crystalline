/**
 * SOUND BOARD: every patch the station plays, one button each, for
 * listening to them (M4 C28).
 *
 * `SOUNDS` (`dev/sounds.ts`) names every effect and every room's drone (`audio/effects.ts`,
 * `audio/ambience.ts`); a click plays the patch on its bus (`busOf`: the
 * drones on `ambience`, everything else on `effects`) through a mixer of
 * the board's own, made and unlocked inside that first click. A looping
 * patch (a drone, the ride's hum) is a toggle: a second click stops it, and
 * `STOP ALL` stops every loop. The unmount stops everything and closes the
 * context. Mute is left out: the board is for hearing.
 *
 * Development only - the route that renders this (`/π/dev/sounds`) exists
 * only under `import.meta.env.DEV`, like the look demo and the gallery.
 */

import { useEffect, useRef, useState } from "react";

import { createMixer, makeAudioContext, type Mixer } from "../audio/mixer";
import { playPatch, type PlayingPatch } from "../audio/synth";
import { C64_BLUE, C64_LIGHT_BLUE } from "../ui/DeviceRefusal";
import { SOUNDS, busOf } from "./sounds";

const LOOPS = new Set(
  Object.entries(SOUNDS)
    .filter(([, make]) => make().loop === true)
    .map(([name]) => name),
);

/** The board. See the module doc. */
export default function SoundBoard() {
  const mixerRef = useRef<Mixer | null>(null);
  const loopsRef = useRef(new Map<string, PlayingPatch>());
  const [held, setHeld] = useState<ReadonlySet<string>>(new Set());

  useEffect(() => {
    const mixer = createMixer({ make: makeAudioContext });
    mixerRef.current = mixer;
    const loops = loopsRef.current;
    return () => {
      for (const playing of loops.values()) playing.stop();
      loops.clear();
      mixerRef.current = null;
      mixer.close();
    };
  }, []);

  const sync = () => setHeld(new Set(loopsRef.current.keys()));

  const play = (name: string) => {
    const mixer = mixerRef.current;
    const make = SOUNDS[name];
    if (mixer === null || make === undefined) return;
    const loops = loopsRef.current;
    const holding = loops.get(name);
    if (holding !== undefined) {
      holding.stop();
      loops.delete(name);
      sync();
      return;
    }
    // Inside the click: makes the context on the first one.
    mixer.unlock();
    const ctx = mixer.ctx;
    const patch = make();
    const bus = mixer.bus(busOf(patch));
    if (ctx === null || bus === null) return;
    const playing = playPatch(ctx, bus, patch, ctx.currentTime, patch.name);
    if (patch.loop === true) {
      loops.set(name, playing);
      sync();
    }
  };

  const stopAll = () => {
    for (const playing of loopsRef.current.values()) playing.stop();
    loopsRef.current.clear();
    sync();
  };

  return (
    <main
      style={{
        minHeight: "100vh",
        padding: "24px 16px",
        background: C64_BLUE,
        color: C64_LIGHT_BLUE,
        fontFamily: "monospace",
      }}
    >
      <h1 style={{ fontSize: 20, margin: "0 0 16px" }}>SOUND BOARD</h1>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
        {Object.keys(SOUNDS).map((name) => {
          const loop = LOOPS.has(name);
          return (
            <button
              key={name}
              type="button"
              onClick={() => {
                play(name);
              }}
              aria-pressed={loop ? held.has(name) : undefined}
              style={buttonStyle(held.has(name))}
            >
              {name}
            </button>
          );
        })}
        <button type="button" onClick={stopAll} style={buttonStyle(false)}>
          STOP ALL
        </button>
      </div>
    </main>
  );
}

function buttonStyle(on: boolean) {
  return {
    padding: "6px 10px",
    fontFamily: "inherit",
    border: `2px solid ${C64_LIGHT_BLUE}`,
    background: on ? C64_LIGHT_BLUE : "transparent",
    color: on ? C64_BLUE : C64_LIGHT_BLUE,
    cursor: "pointer",
  } as const;
}
