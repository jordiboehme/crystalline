/**
 * Vertex arrays for the room mesh and for full-screen passes.
 *
 * The room is one interleaved buffer in the layout `render/geometry.ts`
 * writes: position (location 0), normal (1), uv (2), layer (3), tint (4),
 * flag (5), 13 floats a vertex. The full-screen passes use no buffer at all:
 * a single triangle whose corners the vertex shader derives from
 * `gl_VertexID`, which covers the screen with one primitive.
 */

import { FLOATS_PER_VERTEX, type MeshData } from "../render/geometry";

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
  const stride = FLOATS_PER_VERTEX * 4;
  for (const [location, size, offset] of ATTRIBUTES) {
    gl.enableVertexAttribArray(location);
    gl.vertexAttribPointer(location, size, gl.FLOAT, false, stride, offset * 4);
  }
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
