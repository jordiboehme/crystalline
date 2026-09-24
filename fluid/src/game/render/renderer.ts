/**
 * Frames from a room: the scene pass, the bloom and the final composite.
 *
 * `setRoom` does the expensive work once per room or look - it builds the
 * mesh, fills the texture array (procedural layers, pictogram, and the text
 * layers of `layerPlan`: one per screen, poster and placard, one per six
 * labels) and keeps the look's numbers - and `draw` is
 * then a handful of uniform uploads and six full-screen passes. The scene is
 * rendered at the canvas size handed to `resize`, the bloom at half of that
 * and below.
 *
 * Every GPU object is owned here and released in `dispose`, which the demo
 * calls on unmount. After a lost context the demo does not call it: the
 * context took every GPU object with it, so the demo drops the renderer and
 * builds a fresh one when the context is restored.
 */

import type { GlCaps } from "../gl/context";
import { mat4, multiply, perspective, fpsView, type Vec3 } from "../gl/math";
import { createFullscreen, createMesh, type Mesh } from "../gl/mesh";
import { createProgram, type Program } from "../gl/program";
import { createTarget, type Target } from "../gl/target";
import { createTextureArray, type TextureArray } from "../gl/textureArray";
import { CELL } from "../world/generate";
import type { RoomSpec } from "../world/types";
import { FLOATS_PER_VERTEX, buildRoomMesh, type MeshData } from "./geometry";
import { LAYER, LAYER_SIZE, layerPlan } from "./layers";
import { C64_PALETTE, applyCondition, type Look } from "./looks";
import {
  BRIGHT_FS,
  COMPOSITE_FS,
  DOWN_FS,
  FULLSCREEN_VS,
  MAX_ZONES,
  SCENE_FS,
  SCENE_VS,
  UP_FS,
} from "./shaders";
import { drawPictogramLayer, drawTextLayers } from "./text";
import { baseLayers } from "./textures";

/**
 * Where the player looks from: the eye in world metres (y up, head bob
 * already added), yaw in radians around the vertical axis and pitch in
 * radians, which the movement code keeps within +-30 degrees.
 */
export interface Camera {
  eye: Vec3;
  yaw: number;
  pitch: number;
}

/**
 * The station's renderer. `setRoom` builds the mesh and texture array for a
 * room in a look (call it again when either changes), `resize` rebuilds the
 * offscreen targets for a new canvas size in device pixels, `draw` renders
 * one frame with the zones' current light levels (DOOM's 0 to 255 scale, in
 * `room.lights` order) and the time in seconds for the portal's swirl, and
 * `dispose` frees every GPU object. `draw` before `setRoom` draws nothing.
 */
export interface Renderer {
  setRoom(room: RoomSpec, look: Look): void;
  resize(width: number, height: number): void;
  draw(camera: Camera, levels: Float32Array, seconds: number): void;
  dispose(): void;
}

/**
 * Several meshes as one vertex array, in order. A bridge until the renderer
 * draws each door's movers on their own: until then the panels are appended
 * to the static room at their closed position.
 */
function joinMeshes(meshes: readonly MeshData[]): MeshData {
  const count = meshes.reduce((n, m) => n + m.count, 0);
  const vertices = new Float32Array(count * FLOATS_PER_VERTEX);
  let at = 0;
  for (const m of meshes) {
    vertices.set(m.vertices, at);
    at += m.vertices.length;
  }
  return { vertices, count };
}

/** Vertical field of view: 70 degrees, a little wider than DOOM's feel on a tall screen. */
const FOV_Y = (70 * Math.PI) / 180;

/**
 * Compiles the five programs and makes the procedural layers once, then
 * returns a renderer bound to `gl`. The offscreen targets use `caps.color`,
 * falling back to RGBA8 per target when the driver refuses a half-float
 * framebuffer, and the texture array is capped at `caps.maxLayers`, so a
 * room with more text than the GPU can hold loses its last labels instead
 * of failing. Throws when a shader does not compile, which is a programming
 * error, or when not even an RGBA8 target can be made.
 */
export function createRenderer(
  gl: WebGL2RenderingContext,
  caps: GlCaps,
): Renderer {
  const scene: Program = createProgram(gl, SCENE_VS, SCENE_FS);
  const bright = createProgram(gl, FULLSCREEN_VS, BRIGHT_FS);
  const down = createProgram(gl, FULLSCREEN_VS, DOWN_FS);
  const up = createProgram(gl, FULLSCREEN_VS, UP_FS);
  const composite = createProgram(gl, FULLSCREEN_VS, COMPOSITE_FS);
  const fullscreen = createFullscreen(gl);
  const base = baseLayers(LAYER_SIZE, 0x5eed);
  const pictogram = drawPictogramLayer(LAYER_SIZE);

  let mesh: Mesh | null = null;
  let textures: TextureArray | null = null;
  let room: RoomSpec | null = null;
  let look: Look | null = null;
  let targets: { scene: Target; down: Target[]; up: Target[] } | null = null;
  let size = { width: 1, height: 1 };
  const zoneRects = new Float32Array(MAX_ZONES * 4);
  const zoneLevels = new Float32Array(MAX_ZONES);
  const projection = mat4();
  const view = mat4();
  const viewProjection = mat4();
  const palette = new Float32Array(C64_PALETTE.flatMap((c) => [...c]));

  const releaseTargets = () => {
    if (targets === null) return;
    targets.scene.dispose();
    for (const t of [...targets.down, ...targets.up]) t.dispose();
    targets = null;
  };

  const makeTarget = (w: number, h: number, depth: boolean): Target => {
    const t =
      createTarget(gl, w, h, caps.color, depth) ??
      createTarget(gl, w, h, "rgba8", depth);
    if (t === null) throw new Error("no render target could be created");
    return t;
  };

  const buildTargets = () => {
    releaseTargets();
    const { width, height } = size;
    targets = {
      scene: makeTarget(width, height, true),
      down: [2, 4, 8].map((d) => makeTarget(width / d, height / d, false)),
      up: [2, 4].map((d) => makeTarget(width / d, height / d, false)),
    };
  };

  const pass = (
    program: Program,
    target: Target | null,
    source: WebGLTexture,
  ) => {
    gl.bindFramebuffer(gl.FRAMEBUFFER, target?.framebuffer ?? null);
    gl.viewport(
      0,
      0,
      target?.width ?? gl.drawingBufferWidth,
      target?.height ?? gl.drawingBufferHeight,
    );
    gl.useProgram(program.program);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, source);
    gl.uniform1i(program.uniform("uSource"), 0);
    if (target !== null) {
      gl.uniform2f(
        program.uniform("uHalfPixel"),
        0.5 / target.width,
        0.5 / target.height,
      );
    }
    fullscreen.draw();
  };

  return {
    setRoom(nextRoom, nextLook) {
      room = nextRoom;
      look = applyCondition(nextLook, nextRoom.condition);
      mesh?.dispose();
      // Bridge until the renderer draws movers: the door panels are drawn
      // closed, appended to the static mesh as one draw call.
      const built = buildRoomMesh(nextRoom, look);
      mesh = createMesh(
        gl,
        joinMeshes([built.static, ...built.movers.map((m) => m.mesh)]),
      );
      textures?.dispose();
      const plan = layerPlan(nextRoom);
      const count = Math.min(plan.count, caps.maxLayers);
      textures = createTextureArray(gl, LAYER_SIZE, count);
      base.forEach((pixels, i) => {
        if (i !== LAYER.pictogram) textures?.setLayer(i, pixels);
      });
      textures.setLayer(LAYER.pictogram, pictogram);
      for (const { layer, pixels } of drawTextLayers(
        plan,
        nextLook,
        LAYER_SIZE,
      )) {
        if (layer < count) textures.setLayer(layer, pixels);
      }
      textures.finish();
      zoneRects.fill(0);
      nextRoom.lights.slice(0, MAX_ZONES).forEach((z, i) => {
        zoneRects.set(
          [z.x0 * CELL, z.y0 * CELL, z.x1 * CELL, z.y1 * CELL],
          i * 4,
        );
      });
    },

    resize(width, height) {
      size = {
        width: Math.max(1, Math.floor(width)),
        height: Math.max(1, Math.floor(height)),
      };
      buildTargets();
    },

    draw(camera, levels, seconds) {
      if (mesh === null || textures === null || room === null || look === null)
        return;
      if (targets === null) buildTargets();
      const t = targets;
      if (t === null) return;
      const zoneCount = Math.min(room.lights.length, MAX_ZONES);
      for (let i = 0; i < zoneCount; i++)
        zoneLevels[i] = (levels[i] ?? 0) / 255;

      perspective(projection, FOV_Y, size.width / size.height, 0.05, 200);
      fpsView(view, camera.eye, camera.yaw, camera.pitch);
      multiply(viewProjection, projection, view);

      // Scene.
      gl.bindFramebuffer(gl.FRAMEBUFFER, t.scene.framebuffer);
      gl.viewport(0, 0, t.scene.width, t.scene.height);
      const fog = look.palette.fog;
      gl.clearColor(fog[0], fog[1], fog[2], 1);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      gl.enable(gl.DEPTH_TEST);
      gl.enable(gl.CULL_FACE);
      gl.useProgram(scene.program);
      gl.uniformMatrix4fv(
        scene.uniform("uViewProjection"),
        false,
        viewProjection,
      );
      gl.uniform3f(
        scene.uniform("uEye"),
        camera.eye[0],
        camera.eye[1],
        camera.eye[2],
      );
      gl.uniform1f(scene.uniform("uTime"), seconds);
      gl.uniform4fv(scene.uniform("uZoneRect"), zoneRects);
      gl.uniform1fv(scene.uniform("uZoneLevel"), zoneLevels);
      gl.uniform1i(scene.uniform("uZoneCount"), zoneCount);
      gl.uniform1f(scene.uniform("uLightScale"), look.lightScale);
      gl.uniform1f(scene.uniform("uFalloff"), look.falloff);
      gl.uniform1f(scene.uniform("uMinLight"), look.minLight);
      gl.uniform1f(scene.uniform("uBands"), look.bands);
      gl.uniform1f(scene.uniform("uGrime"), look.grime);
      gl.uniform1f(scene.uniform("uGrimeLayer"), LAYER.grime);
      gl.uniform1f(scene.uniform("uTextureMix"), look.textureMix);
      gl.uniform3f(scene.uniform("uEdgeColour"), ...look.edge.colour);
      gl.uniform1f(scene.uniform("uEdgeStrength"), look.edge.strength);
      gl.uniform1f(scene.uniform("uEdgeWidth"), look.edge.width);
      gl.uniform1i(
        scene.uniform("uEdgeEverywhere"),
        look.edge.everywhere ? 1 : 0,
      );
      gl.uniform1f(scene.uniform("uLdr"), caps.color === "rgba8" ? 1 : 0);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D_ARRAY, textures.texture);
      gl.uniform1i(scene.uniform("uTextures"), 0);
      mesh.draw();
      gl.disable(gl.DEPTH_TEST);
      gl.disable(gl.CULL_FACE);

      // Bloom: bright pass, two Kawase steps down, two up.
      const [d0, d1, d2] = t.down;
      const [u0, u1] = t.up;
      if (d0 && d1 && d2 && u0 && u1) {
        gl.useProgram(bright.program);
        gl.uniform1f(
          bright.uniform("uThreshold"),
          caps.color === "rgba8" ? 0.75 : look.bloom.threshold,
        );
        pass(bright, d0, t.scene.texture);
        pass(down, d1, d0.texture);
        pass(down, d2, d1.texture);
        pass(up, u1, d2.texture);
        pass(up, u0, u1.texture);

        // Composite to the canvas.
        gl.bindFramebuffer(gl.FRAMEBUFFER, null);
        gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
        gl.useProgram(composite.program);
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, t.scene.texture);
        gl.uniform1i(composite.uniform("uScene"), 0);
        gl.activeTexture(gl.TEXTURE1);
        gl.bindTexture(gl.TEXTURE_2D, u0.texture);
        gl.uniform1i(composite.uniform("uBloom"), 1);
        gl.uniform1f(composite.uniform("uBloomStrength"), look.bloom.strength);
        gl.uniform1i(
          composite.uniform("uToneMap"),
          caps.color === "rgba16f" ? 1 : 0,
        );
        gl.uniform1i(composite.uniform("uDither"), look.dither ? 1 : 0);
        gl.uniform3fv(composite.uniform("uPalette"), palette);
        fullscreen.draw();
        // Unbind the bloom from unit 1: the next frame renders into it.
        gl.bindTexture(gl.TEXTURE_2D, null);
        gl.activeTexture(gl.TEXTURE0);
      }
    },

    dispose() {
      releaseTargets();
      mesh?.dispose();
      textures?.dispose();
      for (const p of [scene, bright, down, up, composite]) p.dispose();
      fullscreen.dispose();
    },
  };
}
