/**
 * The station's GLSL: one shader for every surface, four small ones for the
 * bloom and the final picture.
 *
 * The surface shader does DOOM's diminishing light: the cell's level minus a
 * term that grows with distance, clamped to the look's floor and quantised
 * into bands, so light falls off in visible steps the way DOOM's COLORMAP
 * did. The level comes from the room's light grid, a small R8 texture with
 * one texel per grid cell that the renderer refills every frame (see
 * `render/lightgrid.ts`), so a room lights any number of zones at the cost
 * of one texture read. On top come the neon edge lines, drawn where a
 * surface's uv (in metres) crosses a whole number, antialiased with the uv's
 * screen-space derivatives; they are added above 1.0 so the bloom picks them
 * up. The portal surface scrolls its swirl layer and brightens towards its
 * rim.
 */

import { CELL } from "../world/generate";

/**
 * How far, in metres, the surface shader moves a fragment along its normal
 * before it picks the light-grid cell: enough to step off a wall's cell
 * border into the floor cell the wall faces, and far below the 0.3 m a
 * flush part stands out from its wall, so no part is lit from a cell it
 * does not stand in.
 */
export const LIGHT_NUDGE = 0.05;

/**
 * The surface vertex shader. It reads the 13-float vertex the geometry
 * writes, at the attribute locations `gl/mesh.ts` binds (position 0, normal
 * 1, uv 2, layer 3, tint 4, flag 5), moves it by `uModelOffset` (a door
 * panel's slide while it opens; zero for the static room), passes the moved
 * world position on for the distance and light-grid lookups, and hands
 * layer, tint and flag through flat so a triangle never blends between two
 * surfaces. The flag is rounded to an int once here, so the fragment shader
 * compares whole numbers.
 */
export const SCENE_VS = `#version 300 es
layout(location = 0) in vec3 aPosition;
layout(location = 1) in vec3 aNormal;
layout(location = 2) in vec2 aUv;
layout(location = 3) in float aLayer;
layout(location = 4) in vec3 aTint;
layout(location = 5) in float aFlag;
uniform mat4 uViewProjection;
uniform vec3 uModelOffset;
out vec3 vWorld;
out vec3 vNormal;
out vec2 vUv;
flat out float vLayer;
flat out vec3 vTint;
flat out int vFlag;
void main() {
  vec3 world = aPosition + uModelOffset;
  vWorld = world;
  vNormal = aNormal;
  vUv = aUv;
  vLayer = aLayer;
  vTint = aTint;
  vFlag = int(aFlag + 0.5);
  gl_Position = uViewProjection * vec4(world, 1.0);
}
`;

/**
 * The surface fragment shader, one for everything in the room. The flag
 * from `FLAG` in `geometry.ts` picks the path: emissive text and screens
 * ignore the light, a lamp glows with its cell's level, the portal scrolls
 * its swirl and brightens at the rim, and lit surfaces (with frames among
 * them) get the banded, distance-dimmed cell light, optional grime and the
 * neon edge lines. On an RGBA8 target (`uLdr` 1) the edge lines are toned
 * down, since nothing above 1.0 survives there and the bloom threshold is
 * lower.
 *
 * The light grid (`uLightGrid`, R8 with NEAREST filtering, `uGridSize`
 * cells wide and deep, row 0 the grid's north row) is read at the centre of
 * the fragment's cell, `floor(world.xz / CELL) + 0.5` over the grid size. A
 * wall lies exactly on the border between its floor cell and the void cell
 * behind it, where `floor` would pick either side, so the point is first
 * nudged `LIGHT_NUDGE` metres along the surface normal, which every wall
 * turns towards its floor cell. A texel of 0 is legal: a dark cell.
 *
 * The uv's screen-space derivatives are taken once, at the top of `main`
 * before any early return, so they are defined for every fragment of the
 * quad; the edge lines and the grime lookup (`textureGrad`) share them.
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
uniform sampler2DArray uTextures;
uniform vec3 uEye;
uniform float uTime;
uniform sampler2D uLightGrid;
uniform vec2 uGridSize;
uniform float uLightScale;
uniform float uFalloff;
uniform float uMinLight;
uniform float uBands;
uniform float uGrime;
uniform float uTextureMix;
uniform vec3 uEdgeColour;
uniform float uEdgeStrength;
uniform float uEdgeWidth;
uniform bool uEdgeEverywhere;
uniform float uGrimeLayer;
uniform float uLdr;
out vec4 outColour;

const float CELL = ${CELL.toFixed(1)};
const float LIGHT_NUDGE = ${LIGHT_NUDGE.toFixed(2)};

float cellLevel(vec3 p, vec3 n) {
  vec2 at = p.xz + normalize(n).xz * LIGHT_NUDGE;
  vec2 cell = floor(at / CELL) + 0.5;
  return texture(uLightGrid, cell / uGridSize).r;
}

float edgeLine(vec2 uv, vec2 fw, float width) {
  vec2 g = abs(fract(uv - 0.5) - 0.5) / max(fw, vec2(1e-4));
  return 1.0 - clamp(min(g.x, g.y) / width, 0.0, 1.0);
}

void main() {
  vec2 dx = dFdx(vUv);
  vec2 dy = dFdy(vUv);
  vec2 fw = abs(dx) + abs(dy);
  vec3 texel = texture(uTextures, vec3(vUv, vLayer)).rgb;
  vec3 base = vTint * mix(vec3(1.0), texel, uTextureMix);
  float level = cellLevel(vWorld, vNormal);

  if (vFlag == 1) {
    outColour = vec4(vTint * texel * 1.4, 1.0);
    return;
  }
  if (vFlag == 4) {
    outColour = vec4(vTint * (0.3 + level * 2.2 * uLightScale), 1.0);
    return;
  }
  if (vFlag == 2) {
    vec2 centred = vUv / vec2(1.7, 2.45) - 0.5;
    float rim = smoothstep(0.25, 0.5, max(abs(centred.x), abs(centred.y)));
    vec2 swirlUv = vUv * 0.5 + vec2(uTime * 0.07, -uTime * 0.11);
    float swirl = texture(uTextures, vec3(swirlUv, vLayer)).r;
    outColour = vec4(vTint * (0.6 + swirl * 0.9 + rim * 1.6), 1.0);
    return;
  }

  if (uGrime > 0.0) {
    float grime = textureGrad(uTextures, vec3(vUv * 0.37, uGrimeLayer), dx * 0.37, dy * 0.37).r;
    base *= 1.0 - uGrime * grime * 0.85;
  }

  // DOOM's diminishing light, then quantised into bands.
  float dist = distance(uEye, vWorld);
  float lit = clamp(level * uLightScale * 1.25 - dist * uFalloff * (1.1 - level), uMinLight, 1.0);
  lit = floor(lit * uBands + 0.5) / uBands;
  // A little fixed shading by face direction, so walls read as walls.
  float facing = 0.82 + 0.18 * abs(dot(normalize(vNormal), normalize(vec3(0.35, 0.8, 0.5))));
  vec3 colour = base * lit * facing;

  bool framed = vFlag == 3 || (uEdgeEverywhere && vFlag == 0);
  if (framed) {
    float e = edgeLine(vUv, fw, uEdgeWidth);
    vec3 edgeColour = vFlag == 3 ? vTint : uEdgeColour;
    colour += edgeColour * e * uEdgeStrength * mix(1.0, 0.45, uLdr);
  }
  outColour = vec4(colour, 1.0);
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
 * headroom above 1), and for Freescape 64 an ordered 4x4 Bayer dither on
 * 2x2 pixel blocks into the sixteen colours of `uPalette`, the C64 palette.
 */
export const COMPOSITE_FS = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uScene;
uniform sampler2D uBloom;
uniform float uBloomStrength;
uniform bool uToneMap;
uniform bool uDither;
uniform vec3 uPalette[16];
out vec4 outColour;

const float BAYER[16] = float[16](
  0.0, 8.0, 2.0, 10.0,
  12.0, 4.0, 14.0, 6.0,
  3.0, 11.0, 1.0, 9.0,
  15.0, 7.0, 13.0, 5.0);

vec3 nearest(vec3 c) {
  vec3 best = uPalette[0];
  float bestD = 1e9;
  for (int i = 0; i < 16; i++) {
    vec3 d = c - uPalette[i];
    float dd = dot(d, d);
    if (dd < bestD) { bestD = dd; best = uPalette[i]; }
  }
  return best;
}

void main() {
  vec3 c = texture(uScene, vUv).rgb + texture(uBloom, vUv).rgb * uBloomStrength;
  if (uToneMap) c = c * 1.6 / (1.0 + c);
  if (uDither) {
    ivec2 p = ivec2(gl_FragCoord.xy) / 2 % 4;
    float t = BAYER[p.y * 4 + p.x] / 16.0 - 0.5;
    c = nearest(clamp(c + t * 0.22, 0.0, 1.0));
  }
  outColour = vec4(clamp(c, 0.0, 1.0), 1.0);
}
`;
