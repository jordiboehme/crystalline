/**
 * The station's GLSL: one shader for every surface, four small ones for the
 * bloom and the final picture.
 *
 * The surface shader does the classic banded distance light: the cell's
 * level minus a term that grows with distance, clamped to the look's floor
 * and quantised into bands, so light falls off in visible steps the way an
 * old palette-based colour map did. The level comes from the room's light
 * grid, a small R8 texture with one texel per grid cell that the renderer
 * refills every frame (see `render/lightgrid.ts`), so a room lights any
 * number of zones at the cost of one texture read. On top come the neon edge
 * lines, drawn where a surface's uv (in metres) crosses a whole number,
 * antialiased with the uv's screen-space derivatives; they are added above
 * 1.0 so the bloom picks them up. The portal surface scrolls its swirl
 * layer and brightens towards its rim.
 *
 * Self-lit lights take their own path: the ceiling panels (`FLAG.lamp`)
 * follow their cell's light, but a signal light (`FLAG.signal`: a door's
 * warning lamp, a hero's steady light) and a blinking one (the blink flags)
 * shine by themselves, as bright in a dark room as in a lit one (H12). A
 * blinking light's gain comes from `uBlink`, the blink banks' gains
 * (`blink.ts`), at the channel its instance slot and its flag name (H11).
 */

import { CELL } from "../world/units";
import { BLINK_CHANNELS, BLINK_GROUPS } from "./blink";
import { FLAG } from "./geometry";
import { turnMat2Columns } from "./kit";
import { ACCENT_COUNT } from "./looks";

/**
 * How far, in metres, the surface shader moves a fragment along its normal
 * before it picks the light-grid cell: enough to step off a wall's cell
 * border into the floor cell the wall faces, and far below the 0.3 m a
 * flush part stands out from its wall, so no part is lit from a cell it
 * does not stand in.
 */
export const LIGHT_NUDGE = 0.05;

/**
 * The scale of the signal and blink branches: a signal light's colour is
 * its tint times this (times its blink gain and `uGain`), so it reads as
 * bright as a screen (the emissive branch's 1.4) whatever the room's light.
 */
export const SIGNAL_GAIN = 1.4;

/**
 * The attribute location of an instance's anchor offset, a `vec3` in world
 * metres. Only a prop's instanced vertex array enables it, with a divisor
 * of one; everything else reads the generic value, which the renderer sets
 * to zero.
 */
export const INSTANCE_OFFSET_LOCATION = 6;

/**
 * The attribute location of an instance's turn and slot, a `vec2`: x the
 * quarter turns, y the slot, the instance's blink bank (H11, `bankSlot` in
 * `blink.ts`): a hero's kind's bank, 0 (the steady bank) for the static
 * room, the movers and the props. It is kept apart from the offset, never
 * packed into one `vec4`, so a disabled attribute's generic (0, 0, 0, 1)
 * cannot leak a 1 into the turn.
 */
export const INSTANCE_TURN_LOCATION = 7;

/**
 * The shader's four quarter turns as GLSL `mat2` literals, emitted from
 * `turnMat2Columns` (and so from `TURN_XZ` in `kit.ts`), the same table
 * the model checks turn the props by, so the GPU and the tests can never
 * disagree on a rotation.
 */
const TURN_TABLE = [0, 1, 2, 3]
  .map(
    (t) =>
      `mat2(${turnMat2Columns(t)
        .map((n) => n.toFixed(1))
        .join(", ")})`,
  )
  .join(", ");

/**
 * The surface vertex shader. It reads the 13-float vertex the geometry
 * writes, at the attribute locations `gl/mesh.ts` binds (position 0, normal
 * 1, uv 2, layer 3, tint 4, flag 5), plus two instance attributes: the
 * anchor offset at location 6 (`INSTANCE_OFFSET_LOCATION`) and the turn
 * and slot at location 7 (`INSTANCE_TURN_LOCATION`). A prop's or a hero's
 * instanced vertex array feeds those once per instance; the static room
 * and the movers leave them disabled, so they read the generic attribute
 * value, which the renderer sets to zero once: offset 0, turn 0 and slot
 * 0, the identity and the steady bank. The slot is the instance's blink
 * bank (H11), passed on flat as `vSlot` for the fragment shader's blink
 * branch; a way's malfunction is drawn with per-draw uniforms, not
 * instance data. The vertex is turned by `TURNS` (emitted from
 * `turnMat2Columns`) and moved by the instance offset; then comes a
 * mover's turn about its pivot's vertical (`uModelYaw`, the cosine and
 * sine of the angle, applied as `mat2(c, -s, s, c)` to the offset from
 * `uModelPivot` in the xz plane: a police box's door leaf swinging on its
 * hinge; (1, 0), no turn, for everything else), its scale about its pivot
 * (`uModelScale` about `uModelPivot`: a portal disc collapsing; 1 about
 * the origin for everything else) and its slide (`uModelOffset`: a door
 * leaf or hatch lid while it opens, the rotor while it rises; zero for
 * the static room and the props). `swungPoint` in `render/parts.ts` is
 * the same turn on the CPU. At those rest values the placement is exact,
 * so the room and the props land where they always did. The normal turns
 * with the vertex, by the instance turn and then the mover's, so a prop
 * is lit and nudged into the light grid the way it faces; a uniform scale
 * does not change a normal's direction. The moved
 * world position goes on for the distance and light-grid lookups, and
 * layer, tint and flag go through flat so a triangle never blends between
 * two surfaces. The flag is rounded to an int once here, so the fragment
 * shader compares whole numbers. The fragment shader is unchanged by
 * instancing, so the light grid, the bands, the grime, the edge lines and
 * the tone map apply to props as to everything else.
 *
 * The tint may carry the accent mark (`ACCENT_MARK` and `accentTint` in
 * `geometry.ts`, 2.7 C8): a negative first channel, which no real colour
 * has, means "the room's accent times the second channel". The shader
 * swaps such a tint for `uAccent` times `aTint.y` before it goes on flat
 * as `vTint`, so the fragment shader only ever sees real colours. The
 * renderer uploads `uAccent` on every draw from the look and the room
 * (`accentFor` in `looks.ts`), so a mesh built once per look takes each
 * room's accent, and a restored context keeps it. The look is fixed in
 * play; only a dev page picks another. A first channel below -1.5 is the
 * prop's own mark (`PROP_MARK`): it takes `uAccents` at the pick the
 * instance's turn float carries above its two turn bits (`propTurn` in
 * `instances.ts`), or `uAccent` with no pick.
 */
export const SCENE_VS = `#version 300 es
layout(location = 0) in vec3 aPosition;
layout(location = 1) in vec3 aNormal;
layout(location = 2) in vec2 aUv;
layout(location = 3) in float aLayer;
layout(location = 4) in vec3 aTint;
layout(location = 5) in float aFlag;
layout(location = 6) in vec3 aInstanceOffset;
// x: quarter turns, y: slot, the blink bank (H11); 0 for the room, the movers
// and the props
layout(location = 7) in vec2 aInstanceTurn;
uniform mat4 uViewProjection;
uniform vec3 uModelOffset;
uniform vec3 uModelPivot;
uniform float uModelScale;
// (cos, sin) of the turn about uModelPivot's vertical; (1, 0) for everything
// but a swinging leaf
uniform vec2 uModelYaw;
// The room's accent (2.7 C8), taken by every tint carrying the accent mark
uniform vec3 uAccent;
// The look's accents, a prop's own accent picked from them (PROP_MARK)
uniform vec3 uAccents[${String(ACCENT_COUNT)}];
out vec3 vWorld;
out vec3 vNormal;
out vec2 vUv;
flat out float vLayer;
flat out vec3 vTint;
flat out int vFlag;
flat out float vSlot;
const mat2 TURNS[4] = mat2[4](${TURN_TABLE});
void main() {
  int turnPick = int(aInstanceTurn.x + 0.5);
  mat2 turn = TURNS[turnPick & 3];
  int pick = turnPick >> 2;
  vec2 xz = turn * aPosition.xz;
  vec3 placed = vec3(xz.x, aPosition.y, xz.y) + aInstanceOffset;
  mat2 yaw = mat2(uModelYaw.x, -uModelYaw.y, uModelYaw.y, uModelYaw.x);
  vec3 local = placed - uModelPivot;
  vec2 turned = yaw * local.xz;
  vec3 world = uModelPivot + vec3(turned.x, local.y, turned.y) * uModelScale + uModelOffset;
  vec2 nxz = yaw * (turn * aNormal.xz);
  vWorld = world;
  vNormal = vec3(nxz.x, aNormal.y, nxz.y);
  vUv = aUv;
  vLayer = aLayer;
  vec3 ownAccent = pick == 0 ? uAccent : uAccents[pick - 1];
  vTint = aTint.x < -1.5 ? ownAccent * aTint.y : aTint.x < 0.0 ? uAccent * aTint.y : aTint;
  vFlag = int(aFlag + 0.5);
  vSlot = aInstanceTurn.y;
  gl_Position = uViewProjection * vec4(world, 1.0);
}
`;

/**
 * The surface fragment shader, one for everything in the room. The flag
 * from `FLAG` in `geometry.ts` picks the path: emissive text and screens
 * ignore the light; a lamp (the ceiling panel's flag) glows with its
 * cell's level and so follows its zone's light; a signal light shines by
 * itself at `SIGNAL_GAIN`, with no room light in it (H12); a blink-flagged
 * light does the same times its channel's gain in `uBlink`, the channel
 * being `vSlot * BLINK_GROUPS + (flag - FLAG.blink)` (H11), and the
 * branch takes only the blink flags, so no flag past them reads outside
 * `uBlink`; the portal
 * scrolls its swirl and brightens at the rim; and lit surfaces (with
 * frames among them) get the banded, distance-dimmed cell light, optional
 * grime and the neon edge lines. On an RGBA8 target (`uLdr` 1) the edge
 * lines are toned down, since nothing above 1.0 survives there and the
 * bloom threshold is lower. `uGain` scales every exit: 1 for the room,
 * the props and the heroes, a fault's flicker, blink or spark for a
 * mover. The blink gain only multiplies it, never replaces it.
 *
 * The light grid (`uLightGrid`, R8 with NEAREST filtering, `uGridSize`
 * cells wide and deep, row 0 the grid's north row) is read at the centre of
 * the fragment's cell, `floor(world.xz / CELL) + 0.5` over the grid size. A
 * wall lies exactly on the border between its floor cell and the void cell
 * behind it, where `floor` would pick either side, so the point is first
 * nudged `LIGHT_NUDGE` metres along the surface normal, which every wall
 * turns towards its floor cell. A texel of 0 is legal: a dark cell.
 *
 * A decal (`FLAG.decal`, 2.7 C20) takes the lit path with no edge lines,
 * after an alpha test: its texel's alpha, against the 4x4 ordered
 * threshold at its pixel (`bayer4`, each step offset half a step so no
 * threshold is 0). A texel below it is discarded, so a hard-edged shape (a
 * chevron, an arrow, a stencil's pixel) stays crisp and a soft one (grime,
 * a streak, rust) fades in an ordered stipple, with no blending and no
 * sorting.
 * The test comes right after the texture reads (the texel, the portal's
 * swirl and the light grid's `cellLevel`), so a discarded fragment writes
 * nothing and every implicit-lod sample is taken while the whole 2x2 quad
 * still runs: a discard is not uniform across a quad, and a sample after
 * it would read undefined derivatives. Everything after the test reads
 * the derivatives taken at the top (`textureGrad` for the grime). The
 * atlas's colour is white, so a decal's colour is its tint.
 *
 * Edge lines: a frame (`FLAG.frame`) always draws them, in its own tint.
 * With `uEdgeEverywhere` the room's shell (`FLAG.shell`) draws
 * them too, in the look's edge colour; a plain `lit` surface (a prop, a
 * hero, a fitting, a terminal) never does. The shell is lit exactly as
 * `lit`: no early return tests its flag. The shell's seams fade with the
 * fragment's distance from the eye over `uEdgeFade` (the look's
 * `edge.fade`, from and to in metres; 0, 0 for none): at full strength
 * nearer than its first value, gone past its second. A frame's lines
 * never fade.
 *
 * The uv's screen-space derivatives are taken once, at the top of `main`
 * before any early return, so they are defined for every fragment of the
 * quad; the edge lines and the grime lookup (`textureGrad`) share them. The
 * portal's swirl is sampled there too, for every fragment, and only used
 * by the portal branch: an implicit-lod `texture()` needs its neighbours'
 * coordinates just as a derivative does, and a branch some fragments of a
 * 2x2 quad take and others do not leaves them undefined.
 */
export const SCENE_FS = `#version 300 es
precision highp float;
precision highp sampler2DArray;
precision highp sampler2D;
in vec3 vWorld;
in vec3 vNormal;
in vec2 vUv;
flat in float vLayer;
flat in vec3 vTint;
flat in int vFlag;
flat in float vSlot;
uniform sampler2DArray uTextures;
uniform vec3 uEye;
uniform float uTime;
uniform float uGain;
uniform sampler2D uLightGrid;
uniform vec2 uGridSize;
uniform float uLightScale;
uniform float uFalloff;
uniform float uMinLight;
uniform float uBands;
uniform float uGrime;
uniform vec3 uEdgeColour;
uniform float uEdgeStrength;
uniform float uEdgeWidth;
uniform bool uEdgeEverywhere;
uniform vec2 uEdgeFade;
uniform float uGrimeLayer;
// The contact shadow texture over the room (shadows.ts), its size in metres
// and how dark it gets (0 none)
uniform sampler2D uShadow;
uniform vec2 uShadowSize;
uniform float uContactShadow;
uniform float uLdr;
uniform float uBlink[${String(BLINK_CHANNELS)}];
out vec4 outColour;

const float CELL = ${CELL.toFixed(1)};
const float LIGHT_NUDGE = ${LIGHT_NUDGE.toFixed(2)};
const float SIGNAL = ${SIGNAL_GAIN.toFixed(2)};
const int BLINK_GROUPS = ${String(BLINK_GROUPS)};

float cellLevel(vec3 p, vec3 n) {
  vec2 at = p.xz + normalize(n).xz * LIGHT_NUDGE;
  vec2 cell = floor(at / CELL) + 0.5;
  return texture(uLightGrid, cell / uGridSize).r;
}

// The 4x4 ordered (Bayer) thresholds a decal's alpha is tested against,
// each offset half a step so the lowest is above 0 and a clear texel is
// always discarded.
const float BAYER4[16] = float[16](
  0.0, 8.0, 2.0, 10.0,
  12.0, 4.0, 14.0, 6.0,
  3.0, 11.0, 1.0, 9.0,
  15.0, 7.0, 13.0, 5.0);

float bayer4(vec2 p) {
  ivec2 q = ivec2(mod(floor(p), 4.0));
  return (BAYER4[q.y * 4 + q.x] + 0.5) / 16.0;
}

float edgeLine(vec2 uv, vec2 fw, float width) {
  vec2 g = abs(fract(uv - 0.5) - 0.5) / max(fw, vec2(1e-4));
  return 1.0 - clamp(min(g.x, g.y) / width, 0.0, 1.0);
}

void main() {
  vec2 dx = dFdx(vUv);
  vec2 dy = dFdy(vUv);
  vec2 fw = abs(dx) + abs(dy);
  vec4 texel4 = texture(uTextures, vec3(vUv, vLayer));
  vec3 texel = texel4.rgb;
  vec2 swirlUv = vUv * 0.5 + vec2(uTime * 0.07, -uTime * 0.11);
  float swirl = texture(uTextures, vec3(swirlUv, vLayer)).r;
  float level = cellLevel(vWorld, vNormal);
  if (vFlag == ${String(FLAG.decal)} && texel4.a < bayer4(gl_FragCoord.xy)) discard;
  vec3 base = vTint * texel;

  if (vFlag == 1) {
    outColour = vec4(vTint * texel * 1.4 * uGain, 1.0);
    return;
  }
  if (vFlag == 4) {
    outColour = vec4(vTint * (0.3 + level * 2.2 * uLightScale) * uGain, 1.0);
    return;
  }
  if (vFlag == ${String(FLAG.signal)}) {
    outColour = vec4(vTint * SIGNAL * uGain, 1.0);
    return;
  }
  if (vFlag >= ${String(FLAG.blink)} && vFlag < ${String(FLAG.blink + BLINK_GROUPS)}) {
    int channel = int(vSlot + 0.5) * BLINK_GROUPS + (vFlag - ${String(FLAG.blink)});
    outColour = vec4(vTint * SIGNAL * uBlink[channel] * uGain, 1.0);
    return;
  }
  if (vFlag == 2) {
    vec2 centred = vUv / vec2(1.7, 2.45) - 0.5;
    float rim = smoothstep(0.25, 0.5, max(abs(centred.x), abs(centred.y)));
    outColour = vec4(vTint * (0.6 + swirl * 0.9 + rim * 1.6) * uGain, 1.0);
    return;
  }

  if (uGrime > 0.0) {
    float grime = textureGrad(uTextures, vec3(vUv * 0.37, uGrimeLayer), dx * 0.37, dy * 0.37).r;
    base *= 1.0 - uGrime * grime * 0.85;
  }

  // The classic banded distance light: fall off, then quantise into bands.
  float dist = distance(uEye, vWorld);
  float lit = clamp(level * uLightScale * 1.25 - dist * uFalloff * (1.1 - level), uMinLight, 1.0);
  if (vNormal.y > 0.9 && vWorld.y < 0.05) lit *= 1.0 - uContactShadow * textureLod(uShadow, vWorld.xz / uShadowSize, 0.0).r;
  lit = floor(lit * uBands + 0.5) / uBands;
  // A little fixed shading by face direction, so walls read as walls.
  float facing = 0.82 + 0.18 * abs(dot(normalize(vNormal), normalize(vec3(0.35, 0.8, 0.5))));
  vec3 colour = base * lit * facing;

  bool framed = vFlag == 3 || (uEdgeEverywhere && vFlag == ${String(FLAG.shell)});
  if (framed) {
    float e = edgeLine(vUv, fw, uEdgeWidth);
    if (vFlag == ${String(FLAG.shell)} && uEdgeFade.y > 0.0) {
      e *= 1.0 - smoothstep(uEdgeFade.x, uEdgeFade.y, dist);
    }
    vec3 edgeColour = vFlag == 3 ? vTint : uEdgeColour;
    colour += edgeColour * e * uEdgeStrength * mix(1.0, 0.45, uLdr);
  }
  outColour = vec4(colour * uGain, 1.0);
}
`;

/** Full-screen triangle from gl_VertexID; uv in [0, 1]. */
export const FULLSCREEN_VS = `#version 300 es
out vec2 vUv;
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  vUv = p;
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
`;

/**
 * The bloom's bright pass: keeps only what peaks above `uThreshold`, with a
 * soft knee so an edge line does not switch its glow on and off as the
 * camera moves.
 */
export const BRIGHT_FS = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uSource;
uniform float uThreshold;
out vec4 outColour;
void main() {
  vec3 c = texture(uSource, vUv).rgb;
  float peak = max(c.r, max(c.g, c.b));
  float keep = smoothstep(uThreshold, uThreshold * 1.25 + 0.05, peak);
  outColour = vec4(c * keep, 1.0);
}
`;

/**
 * One Dual Kawase downsample step: the centre tap weighted four and four
 * diagonal taps half a source texel out, which linear filtering turns into
 * a wide, cheap blur at half the resolution. `uHalfPixel` is half a texel of
 * the target being drawn.
 */
export const DOWN_FS = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uSource;
uniform vec2 uHalfPixel;
out vec4 outColour;
void main() {
  vec3 sum = texture(uSource, vUv).rgb * 4.0;
  sum += texture(uSource, vUv - uHalfPixel).rgb;
  sum += texture(uSource, vUv + uHalfPixel).rgb;
  sum += texture(uSource, vUv + vec2(uHalfPixel.x, -uHalfPixel.y)).rgb;
  sum += texture(uSource, vUv - vec2(uHalfPixel.x, -uHalfPixel.y)).rgb;
  outColour = vec4(sum / 8.0, 1.0);
}
`;

/**
 * One Dual Kawase upsample step: eight taps in a diamond around the pixel,
 * the diagonal ones weighted two, which spreads the downsampled glow back
 * out without blocky steps. `uHalfPixel` is half a texel of the target
 * being drawn.
 */
export const UP_FS = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uSource;
uniform vec2 uHalfPixel;
out vec4 outColour;
void main() {
  vec2 h = uHalfPixel;
  vec3 sum = texture(uSource, vUv + vec2(-h.x * 2.0, 0.0)).rgb;
  sum += texture(uSource, vUv + vec2(-h.x, h.y)).rgb * 2.0;
  sum += texture(uSource, vUv + vec2(0.0, h.y * 2.0)).rgb;
  sum += texture(uSource, vUv + vec2(h.x, h.y)).rgb * 2.0;
  sum += texture(uSource, vUv + vec2(h.x * 2.0, 0.0)).rgb;
  sum += texture(uSource, vUv + vec2(h.x, -h.y)).rgb * 2.0;
  sum += texture(uSource, vUv + vec2(0.0, -h.y * 2.0)).rgb;
  sum += texture(uSource, vUv + vec2(-h.x, -h.y)).rgb * 2.0;
  outColour = vec4(sum / 12.0, 1.0);
}
`;

/**
 * The final picture: scene plus the bloom scaled by its strength, a gentle
 * Reinhard tone map on the half-float path (scaled by 1.6, so a value of
 * 0.6 maps to itself, darker values are lifted a little, 1 comes out at 0.8
 * and full white is only reached at about 1.67, which leaves the bloom
 * headroom above 1).
 */
export const COMPOSITE_FS = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uScene;
uniform sampler2D uBloom;
uniform float uBloomStrength;
uniform bool uToneMap;
out vec4 outColour;

void main() {
  vec3 c = texture(uScene, vUv).rgb + texture(uBloom, vUv).rgb * uBloomStrength;
  if (uToneMap) c = c * 1.6 / (1.0 + c);
  outColour = vec4(clamp(c, 0.0, 1.0), 1.0);
}
`;
