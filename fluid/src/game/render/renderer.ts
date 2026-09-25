/**
 * Frames from a room: the scene pass, the bloom and the final composite.
 *
 * `setRoom` does the expensive work once per room or look - it builds the
 * static room mesh and one small mesh per moving door panel, fills the
 * texture array (procedural layers, the pictogram set, and the text layers
 * of `layerPlan`: one per screen, poster and placard, one per six labels),
 * makes the room's light grid texture, uploads the set dressing and keeps
 * the look's numbers - and `draw` is then a handful of uniform uploads, one
 * small light upload, the room and its door panels, one instanced draw per
 * prop kind and variant, and six full-screen passes. The scene is rendered
 * at the canvas size handed to `resize`, the bloom at half of that and
 * below.
 *
 * The set dressing is drawn instanced (see `instances.ts`). Each prop kind
 * and variant the room needs is built once as its own mesh in the look's
 * colours and kept in a cache keyed by kind and variant; the cache is
 * cleared when the look's id changes and otherwise grows lazily, bounded by
 * the prop catalogue. The condition does not enter the key: it changes
 * only grime and light scale, never the palette a mesh is coloured from (a
 * look test pins that). The room's instance buffers depend only on the
 * room, so a look switch, which calls `setRoom` again with the very same
 * room object, keeps them and only rebuilds the small vertex arrays that
 * bind them to the new look's meshes.
 *
 * Every GPU object is owned here and released in `dispose`, which the demo
 * calls on unmount. After a lost context the demo does not call it: the
 * context took every GPU object with it, so the demo drops the renderer and
 * builds a fresh one when the context is restored.
 */

import type { GlCaps } from "../gl/context";
import { mat4, multiply, perspective, fpsView, type Vec3 } from "../gl/math";
import {
  createFullscreen,
  createInstanceBuffer,
  createInstancedMesh,
  createMesh,
  createVertexBuffer,
  type InstanceBuffer,
  type InstancedMesh,
  type Mesh,
  type VertexBuffer,
} from "../gl/mesh";
import { createProgram, type Program } from "../gl/program";
import { createTarget, type Target } from "../gl/target";
import { createTextureArray, type TextureArray } from "../gl/textureArray";
import type { PropKind, RoomSpec } from "../world/types";
import { buildRoomMesh, type MeshData, type V3 } from "./geometry";
import { propInstances } from "./instances";
import { LAYER, LAYER_SIZE, layerPlan } from "./layers";
import { fillLightTexels, lightGrid, type LightGrid } from "./lightgrid";
import { C64_PALETTE, applyCondition, type Look, type LookId } from "./looks";
import { buildPropMesh } from "./models/props";
import {
  BRIGHT_FS,
  COMPOSITE_FS,
  DOWN_FS,
  FULLSCREEN_VS,
  INSTANCE_OFFSET_LOCATION,
  INSTANCE_TURN_LOCATION,
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
 * The station's renderer.
 *
 * - `setRoom` builds the meshes, the texture array and the light grid for a
 *   room in a look; call it again when either changes. It throws when the
 *   room needs more texture layers than the GPU holds (`caps.maxLayers`,
 *   at least 256 in WebGL2), a limit error like a failed shader: a shorter
 *   array would make the shader clamp the missing layers to the last one
 *   and show readable, wrong labels. That throw, and one from building the
 *   meshes (the room's and any prop model the cache lacks), come before
 *   the old room is released, so the old room stays drawn; only a failure
 *   on the GPU itself leaves nothing to draw. Called again with the same
 *   room object, it keeps the room's prop instance buffers.
 * - `resize` rebuilds the offscreen targets for a new canvas size in device
 *   pixels.
 * - `draw` renders one frame: `levels` holds the zones' current light
 *   levels (DOOM's 0 to 255 scale, in `room.lights` order), `seconds` the
 *   time for the portal's swirl, and `doors` each moving door's open
 *   fraction by its mover key (`door:<fixtureIndex>`), 0 closed to 1 open;
 *   a key that is missing is a closed door, and a fraction outside 0..1 is
 *   clamped.
 * - `dispose` frees every GPU object and may be called more than once.
 *
 * `draw` before `setRoom` or after `dispose` draws nothing.
 */
export interface Renderer {
  setRoom(room: RoomSpec, look: Look): void;
  resize(width: number, height: number): void;
  draw(
    camera: Camera,
    levels: Float32Array,
    seconds: number,
    doors: ReadonlyMap<string, number>,
  ): void;
  dispose(): void;
}

/** A door panel on the GPU: its mesh, key and slide (`axis * travel`). */
interface GpuMover {
  key: string;
  mesh: Mesh;
  slide: V3;
}

/**
 * One prop kind and variant of the room on the GPU: its instance buffer,
 * kept while the room stays the same, and the vertex array that binds it to
 * the cached mesh of the current look, remade on every `setRoom`.
 */
interface GpuPropGroup {
  key: string;
  kind: PropKind;
  variant: number;
  instances: InstanceBuffer;
  mesh: InstancedMesh | null;
}

/**
 * The room's light on the GPU: an R8 texture of one texel per grid cell,
 * the grid that maps cells to zones, and the byte array refilled from the
 * zones' levels before every upload.
 */
interface GpuLight {
  texture: WebGLTexture;
  grid: LightGrid;
  texels: Uint8Array;
}

/** The offscreen targets: the scene, three bloom steps down and two up. */
interface Targets {
  scene: Target;
  d0: Target;
  d1: Target;
  d2: Target;
  u0: Target;
  u1: Target;
}

/** Vertical field of view: 70 degrees, a little wider than DOOM's feel on a tall screen. */
const FOV_Y = (70 * Math.PI) / 180;

/**
 * The near clip plane in metres. Flush door and portal parts stand up to
 * 0.3 m out from their wall and the eye stops 0.35 m from it, so the plane
 * must sit well inside 0.05 m or a face-to-the-wall view clips them.
 */
const NEAR = 0.02;

/** The far clip plane in metres, past the far end of the largest hub. */
const FAR = 200;

/**
 * Compiles the five programs and makes the procedural layers once, then
 * returns a renderer bound to `gl`. The offscreen targets use `caps.color`,
 * falling back to RGBA8 per target when the driver refuses a half-float
 * framebuffer. Throws when a shader does not compile, which is a
 * programming error, or when not even an RGBA8 target can be made.
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

  let disposed = false;
  let mesh: Mesh | null = null;
  let movers: GpuMover[] = [];
  let textures: TextureArray | null = null;
  let light: GpuLight | null = null;
  let room: RoomSpec | null = null;
  let look: Look | null = null;
  let propLook: LookId | null = null;
  const propMeshes = new Map<string, VertexBuffer>();
  let groups: GpuPropGroup[] = [];
  let targets: Targets | null = null;
  let size = { width: 1, height: 1 };
  const projection = mat4();
  const view = mat4();
  const viewProjection = mat4();
  const palette = new Float32Array(C64_PALETTE.flatMap((c) => [...c]));
  // The static room and the door movers leave the instance attributes
  // disabled, so they read these generic values: offset 0 and turn 0.
  gl.vertexAttrib3f(INSTANCE_OFFSET_LOCATION, 0, 0, 0);
  gl.vertexAttrib2f(INSTANCE_TURN_LOCATION, 0, 0);

  const releaseTargets = () => {
    if (targets === null) return;
    const { scene: s, d0, d1, d2, u0, u1 } = targets;
    for (const t of [s, d0, d1, d2, u0, u1]) t.dispose();
    targets = null;
  };

  const releaseRoom = () => {
    mesh?.dispose();
    mesh = null;
    for (const m of movers) m.mesh.dispose();
    movers = [];
    textures?.dispose();
    textures = null;
    if (light !== null) gl.deleteTexture(light.texture);
    light = null;
    room = null;
    look = null;
  };

  /** Deletes every group's vertex array; the instance buffers stay. */
  const releaseGroupMeshes = () => {
    for (const g of groups) {
      g.mesh?.dispose();
      g.mesh = null;
    }
  };

  /** Deletes every group's vertex array and instance buffer. */
  const releaseGroups = () => {
    releaseGroupMeshes();
    for (const g of groups) g.instances.dispose();
    groups = [];
  };

  /** Deletes every cached prop mesh. */
  const releasePropMeshes = () => {
    for (const v of propMeshes.values()) v.dispose();
    propMeshes.clear();
    propLook = null;
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
      d0: makeTarget(width / 2, height / 2, false),
      d1: makeTarget(width / 4, height / 4, false),
      d2: makeTarget(width / 8, height / 8, false),
      u0: makeTarget(width / 2, height / 2, false),
      u1: makeTarget(width / 4, height / 4, false),
    };
  };

  /**
   * The room's light grid texture: immutable R8 storage of one texel per
   * cell, NEAREST so a cell never blends into its neighbour, clamped so a
   * point nudged past the grid's edge reads the edge cell.
   */
  const makeLight = (nextRoom: RoomSpec): GpuLight => {
    const grid = lightGrid(nextRoom);
    const texture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.R8, grid.width, grid.depth);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.bindTexture(gl.TEXTURE_2D, null);
    return { texture, grid, texels: new Uint8Array(grid.width * grid.depth) };
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
      if (disposed) return;
      const plan = layerPlan(nextRoom);
      if (plan.count > caps.maxLayers) {
        throw new Error(
          `room needs ${plan.count} texture layers, the GPU holds ${caps.maxLayers}`,
        );
      }
      // The meshes are built before the old room is let go, so a room that
      // cannot be built leaves the old one on the GPU and drawn.
      const nextLookApplied = applyCondition(nextLook, nextRoom.condition);
      const built = buildRoomMesh(nextRoom, nextLookApplied);
      // A look switch hands the very same room back: its instance buffers
      // stay. This is read before anything is released.
      const sameRoom = nextRoom === room;
      const lookChanged = nextLook.id !== propLook;
      const nextGroups = sameRoom ? null : propInstances(nextRoom);
      const needed: { key: string; kind: PropKind; variant: number }[] =
        nextGroups ?? groups;
      // The prop meshes this room lacks in this look are built on the CPU
      // before the old room is let go, like the room mesh, so a prop that
      // cannot be built leaves the old room drawn too.
      const fresh = new Map<string, MeshData>();
      for (const g of needed) {
        if (!lookChanged && propMeshes.has(g.key)) continue;
        fresh.set(g.key, buildPropMesh(g.kind, g.variant, nextLook));
      }
      releaseRoom();
      releaseGroupMeshes();
      if (lookChanged) {
        releasePropMeshes();
        propLook = nextLook.id;
      }
      if (nextGroups !== null) {
        releaseGroups();
        groups = nextGroups.map((g) => ({
          key: g.key,
          kind: g.kind,
          variant: g.variant,
          instances: createInstanceBuffer(gl, g.data),
          mesh: null,
        }));
      }
      for (const g of groups) {
        let vertices = propMeshes.get(g.key);
        if (vertices === undefined) {
          // Every mesh the cache lacks was built above, before the release;
          // building one here would break the old-room guarantee.
          const data = fresh.get(g.key);
          if (data === undefined) {
            throw new Error(
              `renderer: prop mesh ${g.key} was not built before the release`,
            );
          }
          vertices = createVertexBuffer(gl, data);
          propMeshes.set(g.key, vertices);
        }
        g.mesh = createInstancedMesh(gl, vertices, g.instances);
      }
      mesh = createMesh(gl, built.static);
      movers = built.movers.map((m) => ({
        key: m.key,
        mesh: createMesh(gl, m.mesh),
        slide: [
          m.axis[0] * m.travel,
          m.axis[1] * m.travel,
          m.axis[2] * m.travel,
        ],
      }));
      const array = createTextureArray(gl, LAYER_SIZE, plan.count);
      textures = array;
      base.forEach((pixels, i) => {
        if (i !== LAYER.pictogram) array.setLayer(i, pixels);
      });
      array.setLayer(LAYER.pictogram, pictogram);
      for (const { layer, pixels } of drawTextLayers(
        plan,
        nextLook,
        LAYER_SIZE,
      )) {
        array.setLayer(layer, pixels);
      }
      array.finish();
      light = makeLight(nextRoom);
      room = nextRoom;
      look = nextLookApplied;
    },

    resize(width, height) {
      if (disposed) return;
      size = {
        width: Math.max(1, Math.floor(width)),
        height: Math.max(1, Math.floor(height)),
      };
      buildTargets();
    },

    draw(camera, levels, seconds, doors) {
      if (
        disposed ||
        mesh === null ||
        textures === null ||
        light === null ||
        room === null ||
        look === null
      )
        return;
      if (targets === null) buildTargets();
      const t = targets;
      if (t === null) return;

      perspective(projection, FOV_Y, size.width / size.height, NEAR, FAR);
      fpsView(view, camera.eye, camera.yaw, camera.pitch);
      multiply(viewProjection, projection, view);

      // The light grid, refilled and uploaded on unit 1. Rows of an R8
      // texture are one byte a texel, so the unpack alignment must be 1
      // for a grid whose width is not a multiple of four.
      fillLightTexels(light.grid, levels, light.texels);
      gl.activeTexture(gl.TEXTURE1);
      gl.bindTexture(gl.TEXTURE_2D, light.texture);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      gl.texSubImage2D(
        gl.TEXTURE_2D,
        0,
        0,
        0,
        light.grid.width,
        light.grid.depth,
        gl.RED,
        gl.UNSIGNED_BYTE,
        light.texels,
      );
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
      // The texture array on unit 0.
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D_ARRAY, textures.texture);

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
      gl.uniform1i(scene.uniform("uTextures"), 0);
      gl.uniform1i(scene.uniform("uLightGrid"), 1);
      gl.uniform2f(
        scene.uniform("uGridSize"),
        light.grid.width,
        light.grid.depth,
      );
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
      const offset = scene.uniform("uModelOffset");
      gl.uniform3f(offset, 0, 0, 0);
      mesh.draw();
      for (const m of movers) {
        const open = Math.min(1, Math.max(0, doors.get(m.key) ?? 0));
        gl.uniform3f(
          offset,
          m.slide[0] * open,
          m.slide[1] * open,
          m.slide[2] * open,
        );
        m.mesh.draw();
      }
      gl.uniform3f(offset, 0, 0, 0);
      for (const g of groups) g.mesh?.draw();
      gl.disable(gl.DEPTH_TEST);
      gl.disable(gl.CULL_FACE);

      // Bloom: bright pass, two Kawase steps down, two up.
      gl.useProgram(bright.program);
      gl.uniform1f(
        bright.uniform("uThreshold"),
        caps.color === "rgba8" ? 0.75 : look.bloom.threshold,
      );
      pass(bright, t.d0, t.scene.texture);
      pass(down, t.d1, t.d0.texture);
      pass(down, t.d2, t.d1.texture);
      pass(up, t.u1, t.d2.texture);
      pass(up, t.u0, t.u1.texture);

      // Composite to the canvas.
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
      gl.useProgram(composite.program);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, t.scene.texture);
      gl.uniform1i(composite.uniform("uScene"), 0);
      gl.activeTexture(gl.TEXTURE1);
      gl.bindTexture(gl.TEXTURE_2D, t.u0.texture);
      gl.uniform1i(composite.uniform("uBloom"), 1);
      gl.uniform1f(composite.uniform("uBloomStrength"), look.bloom.strength);
      gl.uniform1i(
        composite.uniform("uToneMap"),
        caps.color === "rgba16f" ? 1 : 0,
      );
      gl.uniform1i(composite.uniform("uDither"), look.dither ? 1 : 0);
      gl.uniform3fv(composite.uniform("uPalette"), palette);
      fullscreen.draw();
      // Unbind the bloom from unit 1 and the scene from unit 0: the next
      // frame renders into both.
      gl.bindTexture(gl.TEXTURE_2D, null);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, null);
    },

    dispose() {
      if (disposed) return;
      disposed = true;
      releaseTargets();
      releaseRoom();
      releaseGroups();
      releasePropMeshes();
      for (const p of [scene, bright, down, up, composite]) p.dispose();
      fullscreen.dispose();
    },
  };
}
