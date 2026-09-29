/**
 * The little linear algebra the station needs, and nothing more.
 *
 * Matrices are column-major `Float32Array`s, the layout `uniformMatrix4fv`
 * takes without transposing. Only three are ever built: a perspective
 * projection, a first-person view from a position, a yaw and a pitch, and
 * their product. There is no general rotation or quaternion here because the
 * camera never rolls and every piece of level geometry is built in world
 * space already.
 *
 * The convention: x east, y up, z south. A yaw of 0 looks north (-z) and a
 * positive yaw turns left, so the mouse moving right lowers the yaw.
 */

/** A point or direction in world space. */
export type Vec3 = readonly [number, number, number];

/** A column-major 4x4 matrix. */
export type Mat4 = Float32Array;

/** Element `i` of a matrix. `noUncheckedIndexedAccess` types every typed array read as possibly missing, and a 4x4 never is. */
function at(m: Mat4, i: number): number {
  return m[i] ?? 0;
}

/** A new identity matrix. */
export function mat4(): Mat4 {
  const m = new Float32Array(16);
  m[0] = 1;
  m[5] = 1;
  m[10] = 1;
  m[15] = 1;
  return m;
}

/** The standard OpenGL perspective projection, depth mapped to [-1, 1]. */
export function perspective(
  out: Mat4,
  fovY: number,
  aspect: number,
  near: number,
  far: number,
): Mat4 {
  const f = 1 / Math.tan(fovY / 2);
  const nf = 1 / (near - far);
  out.fill(0);
  out[0] = f / aspect;
  out[5] = f;
  out[10] = (far + near) * nf;
  out[11] = -1;
  out[14] = 2 * far * near * nf;
  return out;
}

/**
 * The view matrix of a first-person camera. Its rows are the camera's right,
 * up and back axes, and the translation moves the world opposite to the eye.
 */
export function fpsView(
  out: Mat4,
  eye: Vec3,
  yaw: number,
  pitch: number,
): Mat4 {
  const cy = Math.cos(yaw);
  const sy = Math.sin(yaw);
  const cp = Math.cos(pitch);
  const sp = Math.sin(pitch);
  // right, up and back (the negated forward (-sy*cp, sp, -cy*cp)).
  const r: Vec3 = [cy, 0, -sy];
  const u: Vec3 = [sy * sp, cp, cy * sp];
  const b: Vec3 = [sy * cp, -sp, cy * cp];
  const dot = (v: Vec3) => v[0] * eye[0] + v[1] * eye[1] + v[2] * eye[2];
  out[0] = r[0];
  out[4] = r[1];
  out[8] = r[2];
  out[12] = -dot(r);
  out[1] = u[0];
  out[5] = u[1];
  out[9] = u[2];
  out[13] = -dot(u);
  out[2] = b[0];
  out[6] = b[1];
  out[10] = b[2];
  out[14] = -dot(b);
  out[3] = 0;
  out[7] = 0;
  out[11] = 0;
  out[15] = 1;
  return out;
}

/** `out = a * b`, so `b` applies first. `out` may alias neither operand. */
export function multiply(out: Mat4, a: Mat4, b: Mat4): Mat4 {
  for (let col = 0; col < 4; col++) {
    for (let row = 0; row < 4; row++) {
      let sum = 0;
      for (let k = 0; k < 4; k++) {
        sum += at(a, k * 4 + row) * at(b, col * 4 + k);
      }
      out[col * 4 + row] = sum;
    }
  }
  return out;
}

/** `m * (p, 1)`, the homogeneous result. Used by the tests and by picking. */
export function transformPoint(
  m: Mat4,
  p: Vec3,
): [number, number, number, number] {
  const [x, y, z] = p;
  return [
    at(m, 0) * x + at(m, 4) * y + at(m, 8) * z + at(m, 12),
    at(m, 1) * x + at(m, 5) * y + at(m, 9) * z + at(m, 13),
    at(m, 2) * x + at(m, 6) * y + at(m, 10) * z + at(m, 14),
    at(m, 3) * x + at(m, 7) * y + at(m, 11) * z + at(m, 15),
  ];
}

/** The horizontal (x, z) direction a yaw walks forward in. */
export function forwardOf(yaw: number): [number, number] {
  return [-Math.sin(yaw), -Math.cos(yaw)];
}

/** The horizontal (x, z) direction a yaw strafes right in. */
export function rightOf(yaw: number): [number, number] {
  return [Math.cos(yaw), -Math.sin(yaw)];
}
