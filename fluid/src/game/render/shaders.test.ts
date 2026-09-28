import { describe, expect, it } from "vitest";

import { BLINK_CHANNELS, BLINK_GROUPS } from "./blink";
import { ACCENT_MARK, FLAG } from "./geometry";
import rendererSource from "./renderer.ts?raw";
import { LOOK_ORDER, LOOKS } from "./looks";
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

it("turns a mover about its pivot with uModelYaw, the map swungPoint mirrors (2.6e C16)", () => {
  // Mutation caught: the uniform declared but not applied, or the matrix
  // written with the other sign, which would swing every leaf outward.
  expect(SCENE_VS).toContain("uniform vec2 uModelYaw;");
  expect(SCENE_VS).toContain(
    "mat2(uModelYaw.x, -uModelYaw.y, uModelYaw.y, uModelYaw.x)",
  );
});

it("swaps the accent mark for uAccent in the vertex shader (2.7 C8)", () => {
  // Mutation caught: the tint passed through untouched (the stripe would
  // draw with a negative red), or the mark tested on the wrong channel.
  expect(SCENE_VS).toContain("uniform vec3 uAccent;");
  expect(SCENE_VS).toContain(
    "vTint = aTint.x < 0.0 ? uAccent * aTint.y : aTint;",
  );
  expect(ACCENT_MARK).toBeLessThan(0);
});

describe("the scene shader's decal branch", () => {
  it("alpha-tests a decal against the ordered threshold and never draws its edges (2.7 C20)", () => {
    // Mutation caught: no discard (every decal an opaque square), the test on
    // the tinted colour instead of the texel's alpha (Freescape would lose
    // the shapes), or edge lines on decals in the Aperture look.
    expect(FLAG.decal).toBe(14);
    expect(SCENE_FS).toContain("float bayer4(vec2 p)");
    expect(SCENE_FS).toMatch(
      /if \(vFlag == 14 && texel4\.a < bayer4\(gl_FragCoord\.xy\)\) discard;/,
    );
    expect(SCENE_FS).toContain(
      "bool framed = vFlag == 3 || (uEdgeEverywhere && vFlag == 15);",
    );
  });

  it("never lets the lowest ordered threshold keep a clear texel, and samples before it discards (2.7 C20)", () => {
    // Mutation caught: thresholds starting at 0, where `0.0 < 0.0` keeps
    // one fragment in sixteen of a decal's clear margin, a dotted square
    // round every decal; or the swirl or the light grid sampled after the
    // discard.
    expect(SCENE_FS).toMatch(/\(BAYER4\[[^\]]+\] \+ 0\.5\) \/ 16\.0/);
    // The swirl's and the light grid's implicit-lod samples are taken
    // before the discard, so no fragment reads one with a discarded
    // neighbour in its quad; and the discard comes before any colour is
    // written.
    for (const sample of [
      "float swirl = texture(",
      "float level = cellLevel(",
    ]) {
      expect(SCENE_FS.indexOf(sample), sample).toBeGreaterThan(0);
      expect(SCENE_FS.indexOf(sample), sample).toBeLessThan(
        SCENE_FS.indexOf("discard;"),
      );
    }
    expect(SCENE_FS.indexOf("discard;")).toBeLessThan(
      SCENE_FS.indexOf("outColour ="),
    );
  });
});

describe("the scene shader's seams in look 2", () => {
  it("draws the everywhere seams on the shell flag only, never on a plain lit surface", () => {
    // Mutation caught: the framed test back on `vFlag == 0` (every prop,
    // hero and fitting would glow again), or the shell's test dropped.
    expect(FLAG.shell).toBe(15);
    expect(SCENE_FS).toContain(
      `bool framed = vFlag == ${String(FLAG.frame)} || (uEdgeEverywhere && vFlag == ${String(FLAG.shell)});`,
    );
    expect(SCENE_FS).not.toMatch(/vFlag == 0\b/);
    expect(SCENE_FS.match(/uEdgeEverywhere &&/g)?.length).toBe(1);
  });

  it("lights the shell exactly as a lit surface: a flag of its own that no early branch takes", () => {
    // Mutation caught: the shell given a flag another surface has (14, the
    // decal, would discard its fragments; 13, a blink group, would draw it
    // as a blinking light).
    const others = Object.entries(FLAG)
      .filter(([name]) => name !== "shell")
      .map(([, flag]) => flag);
    expect(others).not.toContain(FLAG.shell);
    expect(
      FLAG.shell >= FLAG.blink && FLAG.shell < FLAG.blink + BLINK_GROUPS,
    ).toBe(false);
    const main = SCENE_FS.slice(SCENE_FS.indexOf("void main()"));
    const lit = main.indexOf("float dist = distance(uEye, vWorld);");
    const first = main.indexOf(`vFlag == ${String(FLAG.shell)}`);
    expect(lit).toBeGreaterThan(0);
    expect(first).toBeGreaterThan(lit);
  });

  it("uploads uEdgeEverywhere from the look, which only look 2 turns on", () => {
    // Mutation caught: the day shift or the third look given seams
    // everywhere, or the uniform fed a constant instead of the look.
    expect(rendererSource).toMatch(
      /scene\.uniform\("uEdgeEverywhere"\),\s*look\.edge\.everywhere \? 1 : 0,/,
    );
    for (const id of LOOK_ORDER)
      expect(LOOKS[id].edge.everywhere, id).toBe(id === "aperture");
  });
});

describe("the fade of look 2's seams with distance", () => {
  it("keeps the fade distances in the look's edge settings: look 2 only, starting near 15 m and gone by 25 m", () => {
    // Mutation caught: look 2's fade dropped (null, the far end washes out
    // again), a fade starting in the near view, or one given to a look
    // without seams everywhere.
    const fade = LOOKS.aperture.edge.fade;
    expect(fade).not.toBeNull();
    expect(fade?.from).toBeGreaterThanOrEqual(15);
    expect(fade?.to).toBeGreaterThan(fade?.from ?? Infinity);
    expect(fade?.to).toBeLessThanOrEqual(25);
    for (const id of LOOK_ORDER.filter((id) => id !== "aperture"))
      expect(LOOKS[id].edge.fade, id).toBeNull();
  });

  it("scales the shell's seam by the distance from the eye, and never a frame's line", () => {
    // Mutation caught: the seam left at full strength whatever the
    // distance (the scaling line dropped), the fade applied to the frames
    // too, or the uniform fed a constant instead of the look's fade.
    expect(SCENE_FS).toContain("uniform vec2 uEdgeFade;");
    const framed = SCENE_FS.slice(SCENE_FS.indexOf("bool framed"));
    expect(framed).toContain(
      `if (vFlag == ${String(FLAG.shell)} && uEdgeFade.y > 0.0) {\n      e *= 1.0 - smoothstep(uEdgeFade.x, uEdgeFade.y, dist);\n    }`,
    );
    expect(framed.indexOf("smoothstep(uEdgeFade")).toBeLessThan(
      framed.indexOf("colour += edgeColour * e"),
    );
    // `dist` is the fragment's distance from the eye, taken before.
    expect(
      SCENE_FS.indexOf("float dist = distance(uEye, vWorld);"),
    ).toBeLessThan(SCENE_FS.indexOf("bool framed"));
    expect(rendererSource).toMatch(
      /scene\.uniform\("uEdgeFade"\),\s*look\.edge\.fade\?\.from \?\? 0,\s*look\.edge\.fade\?\.to \?\? 0,/,
    );
  });
});
