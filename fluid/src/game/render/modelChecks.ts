/**
 * The model checks the model tests share: test support, which only tests
 * import. Nothing in the game reads it, so it never reaches a bundle.
 *
 * The fixture and decor tests (`models.test.ts`), the prop tests
 * (`propModels.test.ts`) and the hero tests (`heroModels.test.ts`) measure
 * a built model the same ways: the winding of every triangle against its
 * stored normal, points in a frame's local terms, points inside a floor
 * box, and whether every glowing part (a screen, a frame, a portal, a
 * signal light or a blinking one) touches a lit host or its wall. A prop
 * or hero mesh, built once at the origin, is turned and placed the way the
 * GPU places an instance (`placeMesh`, `placeParts`) before it is measured. The glow check works on a list of recorded kit
 * calls (`Part`), which `recordingKitAt` records for a model built through
 * a kit factory, and which the fixture tests record for the movers too.
 */

import type { Box } from "../world/types";
import { BLINK_GROUPS } from "./blink";
import {
  FLAG,
  FLOATS_PER_VERTEX,
  type Builder,
  type MeshData,
  type Surface,
  type V3,
} from "./geometry";
import { createKit, turnPoint, type Frame, type Kit } from "./kit";
import type { KitAt } from "./models";

/**
 * One kit call, as a recording kit saw it: the builder it emitted into
 * (the room's, or a mover's own), the primitive's name, the layer and flag
 * of its surface (-1 where it took none) and the world positions of the
 * vertices it emitted, three per triangle.
 */
export interface Part {
  builder: object;
  method: string;
  layer: number;
  flag: number;
  points: V3[];
}

/** Every vertex position of a mesh, in order. */
export function positions(m: MeshData): V3[] {
  return Array.from({ length: m.count }, (_, i) => {
    const o = i * FLOATS_PER_VERTEX;
    const v = (k: number) => m.vertices[o + k] ?? NaN;
    return [v(0), v(1), v(2)];
  });
}

/** Every vertex normal of a mesh, in order. */
export function normals(m: MeshData): V3[] {
  return Array.from({ length: m.count }, (_, i) => {
    const o = i * FLOATS_PER_VERTEX + 3;
    const v = (k: number) => m.vertices[o + k] ?? NaN;
    return [v(0), v(1), v(2)];
  });
}

/** `p + q`. */
export const add = (p: V3, q: V3): V3 => [
  p[0] + q[0],
  p[1] + q[1],
  p[2] + q[2],
];
/** `p - q`. */
export const sub = (p: V3, q: V3): V3 => [
  p[0] - q[0],
  p[1] - q[1],
  p[2] - q[2],
];
/** `p * k`. */
export const scale = (p: V3, k: number): V3 => [p[0] * k, p[1] * k, p[2] * k];
/** The cross product `p x q`. */
export const cross = (p: V3, q: V3): V3 => [
  p[1] * q[2] - p[2] * q[1],
  p[2] * q[0] - p[0] * q[2],
  p[0] * q[1] - p[1] * q[0],
];
/** The dot product `p . q`. */
export const dot = (p: V3, q: V3) => p[0] * q[0] + p[1] * q[1] + p[2] * q[2];

/**
 * The smallest agreement between a triangle's winding and its stored
 * normal: the cosine between the stored normal and the geometric normal of
 * the counter-clockwise order, over every triangle of the mesh. 1 is a
 * perfect mesh; anything below 0.999 has a triangle wound the wrong way
 * or a normal that does not match its face. Degenerate triangles are
 * skipped.
 */
export function worstWinding(m: MeshData): number {
  const ps = positions(m);
  const ns = normals(m);
  let worst = Infinity;
  for (let t = 0; t + 2 < ps.length; t += 3) {
    const [a, b, c] = [ps[t], ps[t + 1], ps[t + 2]];
    const n = ns[t];
    if (!a || !b || !c || !n) throw new Error("short triangle");
    const g = cross(sub(b, a), sub(c, a));
    const len = Math.hypot(...g);
    if (len < 1e-9) continue;
    worst = Math.min(worst, dot(g, n) / len);
  }
  return worst;
}

/** A world point in a frame's local `[a, d, h]`. */
export function toLocal(f: Frame, p: V3): V3 {
  const o = sub(p, f.origin);
  return [dot(o, f.along), dot(o, f.inward), o[1]];
}

/**
 * Whether a world point stands over a floor box, within `eps` metres
 * (height is not looked at).
 */
export const inBox = (b: Box, p: V3, eps = 1e-4) =>
  p[0] >= b.x0 - eps &&
  p[0] <= b.x1 + eps &&
  p[2] >= b.z0 - eps &&
  p[2] <= b.z1 + eps;

/** The closest point on triangle `a b c` to `p` (Ericson's method). */
export function closestOnTriangle(p: V3, a: V3, b: V3, c: V3): V3 {
  const ab = sub(b, a);
  const ac = sub(c, a);
  const ap = sub(p, a);
  const d1 = dot(ab, ap);
  const d2 = dot(ac, ap);
  if (d1 <= 0 && d2 <= 0) return a;
  const bp = sub(p, b);
  const d3 = dot(ab, bp);
  const d4 = dot(ac, bp);
  if (d3 >= 0 && d4 <= d3) return b;
  const vc = d1 * d4 - d3 * d2;
  if (vc <= 0 && d1 >= 0 && d3 <= 0) return add(a, scale(ab, d1 / (d1 - d3)));
  const cp = sub(p, c);
  const d5 = dot(ab, cp);
  const d6 = dot(ac, cp);
  if (d6 >= 0 && d5 <= d6) return c;
  const vb = d5 * d2 - d1 * d6;
  if (vb <= 0 && d2 >= 0 && d6 <= 0) return add(a, scale(ac, d2 / (d2 - d6)));
  const va = d3 * d6 - d5 * d4;
  if (va <= 0 && d4 - d3 >= 0 && d5 - d6 >= 0) {
    return add(b, scale(sub(c, b), (d4 - d3) / (d4 - d3 + (d5 - d6))));
  }
  const denom = 1 / (va + vb + vc);
  return add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)));
}

/** How close two parts must come to count as touching, in metres. */
const CONTACT = 0.03;

/** A part's points with their bounds, measured once. */
export interface Shape {
  points: readonly V3[];
  lo: V3;
  hi: V3;
}

/** A shape of a list of points: the points and their bounding box. */
export function shape(points: readonly V3[]): Shape {
  const lo: V3 = [Infinity, Infinity, Infinity];
  const hi: V3 = [-Infinity, -Infinity, -Infinity];
  for (const p of points) {
    for (const k of [0, 1, 2] as const) {
      lo[k] = Math.min(lo[k], p[k]);
      hi[k] = Math.max(hi[k], p[k]);
    }
  }
  return { points, lo, hi };
}

/** Whether a point lies within `CONTACT` of a shape's bounds. */
const nearBounds = (p: V3, s: Shape) =>
  ([0, 1, 2] as const).every(
    (k) => p[k] >= s.lo[k] - CONTACT && p[k] <= s.hi[k] + CONTACT,
  );

/**
 * Whether any vertex of `from` lies within `CONTACT` (3 cm) of a triangle
 * of `to`. Not symmetric: call it both ways round to ask whether two
 * shapes touch.
 */
export function reaches(from: Shape, to: Shape): boolean {
  const near = from.points.filter((p) => nearBounds(p, to));
  if (near.length === 0) return false;
  const pts = to.points;
  for (let t = 0; t + 2 < pts.length; t += 3) {
    const [a, b, c] = [pts[t], pts[t + 1], pts[t + 2]];
    if (!a || !b || !c) continue;
    for (const p of near) {
      const q = closestOnTriangle(p, a, b, c);
      if (Math.hypot(...sub(p, q)) <= CONTACT) return true;
    }
  }
  return false;
}

/**
 * The flags the glow check looks at: emissive, frame, portal, signal and
 * every blink group's (`FLAG.blink` to `FLAG.blink + BLINK_GROUPS - 1`).
 * A lamp (the ceiling panels' flag) is not among them: a panel is part of
 * the ceiling, never a model's light.
 */
export const GLOWING: readonly number[] = [
  FLAG.emissive,
  FLAG.frame,
  FLAG.portal,
  FLAG.signal,
  ...Array.from({ length: BLINK_GROUPS }, (_, g) => FLAG.blink + g),
];

/**
 * Every glowing part that floats: a part with a flag in `GLOWING` that is
 * in contact with no lit host part (a vertex of one within
 * `CONTACT` of a triangle of the other, either way round) and, when `wall`
 * is given, does not reach within `CONTACT` of the wall plane either.
 * Returns `"<index>:<method>"` for each, so the list must be empty: nothing
 * glows in mid-air. Pass every part of the model, a fixture's movers
 * included, since mover parts count as hosts too.
 *
 * A frame is a lit body whose edges glow (the shader lights it like any lit
 * surface and adds its edge lines), so a frame part hosts every other
 * glowing part: a door's warning lamp, a signal light, sits on the frame
 * of its jamb. A frame itself still needs a lit host or the wall.
 */
export function floatingGlow(
  parts: readonly Part[],
  wall: Frame | null,
): string[] {
  const solid = parts.filter((p) => p.points.length > 0);
  const hosts = solid
    .filter((p) => !GLOWING.includes(p.flag))
    .map((p) => shape(p.points));
  const frames = solid
    .filter((p) => p.flag === FLAG.frame)
    .map((p) => shape(p.points));
  return parts
    .map((p, i) => ({ p, i }))
    .filter(({ p }) => GLOWING.includes(p.flag) && p.points.length > 0)
    .filter(({ p }) => {
      if (wall && p.points.some((q) => toLocal(wall, q)[1] <= CONTACT))
        return false;
      const glow = shape(p.points);
      const own = p.flag === FLAG.frame ? hosts : [...hosts, ...frames];
      return !own.some((h) => reaches(glow, h) || reaches(h, glow));
    })
    .map(({ p, i }) => `${String(i)}:${p.method}`);
}

/**
 * A mesh turned and placed as the GPU places an instance: every position
 * turned by `turnPoint` (quarter turns `t`) and moved to the anchor `at`,
 * every normal turned. A prop or hero mesh is built at the origin, so this
 * is where the model tests see it standing in a room.
 */
export function placeMesh(m: MeshData, t: number, at: V3): MeshData {
  const vertices = Float32Array.from(m.vertices);
  for (let i = 0; i < m.count; i++) {
    const o = i * FLOATS_PER_VERTEX;
    const v = (k: number) => vertices[o + k] ?? NaN;
    const p = add(turnPoint([v(0), v(1), v(2)], t), at);
    const n = turnPoint([v(3), v(4), v(5)], t);
    vertices.set([...p, ...n], o);
  }
  return { vertices, count: m.count };
}

/**
 * Recorded parts turned and placed the same way as `placeMesh` places
 * their mesh, so the glow check sees them where the instance stands.
 */
export const placeParts = (parts: readonly Part[], t: number, at: V3): Part[] =>
  parts.map((p) => ({
    ...p,
    points: p.points.map((q) => add(turnPoint(q, t), at)),
  }));

type Fn = (...args: unknown[]) => void;

/** Calls a kit primitive by name. */
const call = (kit: Kit, name: string, args: unknown[]) => {
  (kit as unknown as Record<string, Fn | undefined>)[name]?.(...args);
};

/**
 * A kit factory that emits into `builder` like `(f) => createKit(builder,
 * f)` and also records every primitive call into `parts`: its name, its
 * surface's layer and flag, and its own vertices, emitted a second time
 * into a scratch builder so each part's points are known apart from the
 * mesh.
 */
export function recordingKitAt(builder: Builder, parts: Part[]): KitAt {
  return (f: Frame): Kit => {
    const kit = createKit(builder, f);
    const wrapped: Record<string, Fn> = {};
    for (const name of Object.keys(kit)) {
      wrapped[name] = (...args: unknown[]) => {
        call(kit, name, args);
        const points: V3[] = [];
        const scratch = {
          vertex: (p: V3) => points.push([p[0], p[1], p[2]]),
        } as unknown as Builder;
        call(createKit(scratch, f), name, args);
        const s = args.find(
          (x): x is Surface =>
            typeof x === "object" && x !== null && "flag" in x,
        );
        parts.push({
          builder,
          method: name,
          layer: s?.layer ?? -1,
          flag: s?.flag ?? -1,
          points,
        });
      };
    }
    return wrapped as unknown as Kit;
  };
}
