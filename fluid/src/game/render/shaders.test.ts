import { describe, expect, it } from "vitest";

import { BLINK_CHANNELS, BLINK_GROUPS } from "./blink";
import { FLAG } from "./geometry";
import rendererSource from "./renderer.ts?raw";
import { SCENE_FS, SCENE_VS, SIGNAL_GAIN } from "./shaders";

describe("the scene shader's blink and signal paths", () => {
  it("forwards the instance slot through a flat varying", () => {
    expect(SCENE_VS).toMatch(/flat out float vSlot;/);
    expect(SCENE_VS).toMatch(/vSlot = aInstanceTurn\.y;/);
    expect(SCENE_FS).toMatch(/flat in float vSlot;/);
    expect(SCENE_FS).toContain(
      `uniform float uBlink[${String(BLINK_CHANNELS)}];`,
    );
  });

  it("keeps the lamp branch on the light grid and the signal and blink branches off it (Review Focus 4)", () => {
    const branch = (flag: string) => {
      const i = SCENE_FS.indexOf(flag);
      expect(i, flag).toBeGreaterThan(0);
      return SCENE_FS.slice(i, SCENE_FS.indexOf("return;", i));
    };
    expect(branch(`vFlag == ${String(FLAG.lamp)}`)).toMatch(/level/);
    const signal = branch(`vFlag == ${String(FLAG.signal)}`);
    expect(signal).not.toMatch(/level|uLightScale/);
    expect(signal).toMatch(/uGain/);
    const blink = branch(`vFlag >= ${String(FLAG.blink)}`);
    expect(blink).not.toMatch(/level|uLightScale/);
    expect(blink).toMatch(/uBlink\[/);
    expect(blink).toMatch(/uGain/);
  });

  it("scales the signal and blink branches by SIGNAL and reads the channel by slot and group", () => {
    expect(SCENE_FS).toContain(
      `const float SIGNAL = ${SIGNAL_GAIN.toFixed(2)};`,
    );
    expect(SCENE_FS).toContain(
      `const int BLINK_GROUPS = ${String(BLINK_GROUPS)};`,
    );
    expect(SCENE_FS).toContain(
      `int channel = int(vSlot + 0.5) * BLINK_GROUPS + (vFlag - ${String(FLAG.blink)});`,
    );
    expect(SCENE_FS).toContain(
      "outColour = vec4(vTint * SIGNAL * uGain, 1.0);",
    );
    expect(SCENE_FS).toContain(
      "outColour = vec4(vTint * SIGNAL * uBlink[channel] * uGain, 1.0);",
    );
  });

  it("bounds the blink branch to the blink flags, so no flag past them reads outside uBlink", () => {
    expect(SCENE_FS).toContain(
      `if (vFlag >= ${String(FLAG.blink)} && vFlag < ${String(FLAG.blink + BLINK_GROUPS)}) {`,
    );
  });

  it("puts the signal and blink branches after the lamp branch and before the lit path", () => {
    const main = SCENE_FS.slice(SCENE_FS.indexOf("void main()"));
    const lamp = main.indexOf(`vFlag == ${String(FLAG.lamp)}`);
    const signal = main.indexOf(`vFlag == ${String(FLAG.signal)}`);
    const blink = main.indexOf(`vFlag >= ${String(FLAG.blink)}`);
    const lit = main.indexOf("DOOM's diminishing light");
    expect(lamp).toBeGreaterThan(0);
    expect(signal).toBeGreaterThan(lamp);
    expect(blink).toBeGreaterThan(signal);
    expect(lit).toBeGreaterThan(blink);
  });
});

describe("the renderer's draw order", () => {
  const draw = rendererSource.slice(
    rendererSource.indexOf("draw(camera, levels"),
  );

  it("uploads the blink gains as uBlink", () => {
    expect(draw).toContain('gl.uniform1fv(scene.uniform("uBlink"), blink);');
  });

  it("resets to restDraw after the movers and after the instance groups", () => {
    // The static room at the identity, the movers with their own uniforms,
    // back to the identity before every instance group (props and heroes),
    // and back to the identity once more at the end of the scene pass, so
    // no later draw inherits a mover's gain.
    const rest = "set(restDraw(seconds));";
    const at = (s: string, from = 0) => draw.indexOf(s, from);
    const first = at(rest);
    const room = at("mesh.draw();", first);
    const movers = at("for (const m of movers)", room);
    const second = at(rest, movers);
    const groups = at("for (const g of groups) g.mesh?.draw();", second);
    const third = at(rest, groups);
    for (const i of [first, room, movers, second, groups, third])
      expect(i).toBeGreaterThanOrEqual(0);
    // Nothing is drawn between the second reset and the groups.
    expect(draw.slice(second + rest.length, groups)).not.toMatch(/\.draw\(/);
    expect(at("gl.disable(gl.DEPTH_TEST);", third)).toBeGreaterThan(third);
  });
});
