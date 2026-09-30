/**
 * Vertex arrays for the room mesh, for instanced props and for full-screen
 * passes.
 *
 * The room is one interleaved buffer in the layout `render/geometry.ts`
 * writes: position (location 0), normal (1), uv (2), layer (3), tint (4),
 * flag (5), 13 floats a vertex. Its vertex array enables only those six, so
 * the instance attributes (6 and 7) stay disabled and read their generic
 * value. A prop is drawn instanced: one vertex buffer per kind, variant and
 * look, one instance buffer per group of the room's props, and a vertex
 * array that binds both, the instance attributes with a divisor of one. The
 * full-screen passes use no buffer at all: a single triangle whose corners
 * the vertex shader derives from `gl_VertexID`, which covers the screen
 * with one primitive.
 */

import { FLOATS_PER_VERTEX, type MeshData } from "../render/geometry";
import { INSTANCE_FLOATS } from "../render/instances";
import {
  INSTANCE_OFFSET_LOCATION,
  INSTANCE_TURN_LOCATION,
} from "../render/shaders";

/**
 * An uploaded mesh: its vertex array, the vertex count and a `draw` that
 * issues the single `drawArrays(TRIANGLES)` with the current program.
 * `dispose` deletes the vertex array and its buffer.
 */
export interface Mesh {
  vao: WebGLVertexArrayObject;
  count: number;
  draw(): void;
  dispose(): void;
}

/** Attribute location, component count and float offset within a vertex. */
const ATTRIBUTES: readonly [location: number, size: number, offset: number][] =
  [
    [0, 3, 0],
    [1, 3, 3],
    [2, 2, 6],
    [3, 1, 8],
    [4, 3, 9],
    [5, 1, 12],
  ];

/**
 * Enables the six vertex attributes and points them into the buffer bound
 * to `ARRAY_BUFFER`, in the vertex array that is bound.
 */
function pointVertexAttributes(gl: WebGL2RenderingContext): void {
  const stride = FLOATS_PER_VERTEX * 4;
  for (const [location, size, offset] of ATTRIBUTES) {
    gl.enableVertexAttribArray(location);
    gl.vertexAttribPointer(location, size, gl.FLOAT, false, stride, offset * 4);
  }
}

/**
 * Uploads `data` into a static buffer and records the six attribute
 * pointers in a vertex array. The shaders declare the same locations with
 * `layout(location = n)`. Leaves no vertex array or buffer bound.
 */
export function createMesh(gl: WebGL2RenderingContext, data: MeshData): Mesh {
  const vao = gl.createVertexArray();
  const buffer = gl.createBuffer();
  gl.bindVertexArray(vao);
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, data.vertices, gl.STATIC_DRAW);
  pointVertexAttributes(gl);
  gl.bindVertexArray(null);
  gl.bindBuffer(gl.ARRAY_BUFFER, null);
  return {
    vao,
    count: data.count,
    draw() {
      gl.bindVertexArray(vao);
      gl.drawArrays(gl.TRIANGLES, 0, data.count);
      gl.bindVertexArray(null);
    },
    dispose() {
      gl.deleteVertexArray(vao);
      gl.deleteBuffer(buffer);
    },
  };
}

/**
 * A prop model's vertices on the GPU, without a vertex array of its own:
 * the buffer, its vertex count and a `dispose` that deletes the buffer.
 * Several instanced meshes may bind it; the renderer's prop cache owns it.
 */
export interface VertexBuffer {
  buffer: WebGLBuffer;
  count: number;
  dispose(): void;
}

/**
 * One group's instance records on the GPU: the buffer, the instance count
 * (`data.length / INSTANCE_FLOATS`) and a `dispose` that deletes the
 * buffer.
 */
export interface InstanceBuffer {
  buffer: WebGLBuffer;
  count: number;
  dispose(): void;
}

/**
 * A vertex buffer and an instance buffer bound together in one vertex
 * array. `draw` issues one `drawArraysInstanced(TRIANGLES)` with the
 * current program; `dispose` deletes only the vertex array, never the two
 * buffers, which their owners free.
 */
export interface InstancedMesh {
  draw(): void;
  dispose(): void;
}

/** Uploads a model's vertices into a static buffer. Leaves nothing bound. */
export function createVertexBuffer(
  gl: WebGL2RenderingContext,
  data: MeshData,
): VertexBuffer {
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, data.vertices, gl.STATIC_DRAW);
  gl.bindBuffer(gl.ARRAY_BUFFER, null);
  return {
    buffer,
    count: data.count,
    dispose: () => gl.deleteBuffer(buffer),
  };
}

/**
 * Uploads a group's instance records (`INSTANCE_FLOATS` floats each) into
 * a static buffer. Leaves nothing bound.
 */
export function createInstanceBuffer(
  gl: WebGL2RenderingContext,
  data: Float32Array,
): InstanceBuffer {
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
  gl.bindBuffer(gl.ARRAY_BUFFER, null);
  return {
    buffer,
    count: data.length / INSTANCE_FLOATS,
    dispose: () => gl.deleteBuffer(buffer),
  };
}

/**
 * Records a vertex array that reads the six vertex attributes from
 * `vertices` and the two instance attributes from `instances`: the offset
 * (floats 0 to 2) at `INSTANCE_OFFSET_LOCATION` and the turn and slot
 * (floats 3 and 4) at `INSTANCE_TURN_LOCATION`, both with a divisor of one,
 * set here inside this vertex array only. Leaves no vertex array or buffer
 * bound.
 */
export function createInstancedMesh(
  gl: WebGL2RenderingContext,
  vertices: VertexBuffer,
  instances: InstanceBuffer,
): InstancedMesh {
  const vao = gl.createVertexArray();
  gl.bindVertexArray(vao);
  gl.bindBuffer(gl.ARRAY_BUFFER, vertices.buffer);
  pointVertexAttributes(gl);
  gl.bindBuffer(gl.ARRAY_BUFFER, instances.buffer);
  const stride = INSTANCE_FLOATS * 4;
  gl.enableVertexAttribArray(INSTANCE_OFFSET_LOCATION);
  gl.vertexAttribPointer(
    INSTANCE_OFFSET_LOCATION,
    3,
    gl.FLOAT,
    false,
    stride,
    0,
  );
  gl.vertexAttribDivisor(INSTANCE_OFFSET_LOCATION, 1);
  gl.enableVertexAttribArray(INSTANCE_TURN_LOCATION);
  gl.vertexAttribPointer(
    INSTANCE_TURN_LOCATION,
    2,
    gl.FLOAT,
    false,
    stride,
    12,
  );
  gl.vertexAttribDivisor(INSTANCE_TURN_LOCATION, 1);
  gl.bindVertexArray(null);
  gl.bindBuffer(gl.ARRAY_BUFFER, null);
  const count = vertices.count;
  const instanceCount = instances.count;
  return {
    draw() {
      gl.bindVertexArray(vao);
      gl.drawArraysInstanced(gl.TRIANGLES, 0, count, instanceCount);
      gl.bindVertexArray(null);
    },
    dispose: () => gl.deleteVertexArray(vao),
  };
}

/**
 * The full-screen triangle for post passes: an empty vertex array and a
 * three-vertex draw. The vertex shader builds the corners from
 * `gl_VertexID`, so no buffer is ever bound.
 */
export function createFullscreen(gl: WebGL2RenderingContext): {
  draw(): void;
  dispose(): void;
} {
  const vao = gl.createVertexArray();
  return {
    draw() {
      gl.bindVertexArray(vao);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      gl.bindVertexArray(null);
    },
    dispose: () => gl.deleteVertexArray(vao),
  };
}
