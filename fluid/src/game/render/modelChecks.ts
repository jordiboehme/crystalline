/**
 * The model checks the model tests share: test support, which only tests
 * import. Nothing in the game reads it, so it never reaches a bundle.
 *
 * The fixture and decor tests (`models.test.ts`), the prop tests
 * (`propModels.test.ts`) and the hero tests (`heroModels.test.ts` and the
 * per-batch hero tests under `models/heroes/`) measure a built model the
 * same ways: the winding of every triangle against its stored normal,
 * points in a frame's local terms, points inside a floor box, whether a
 * catalogue surface sits on a real upward face and stays clear above it
 * (`upwardFaceAt`, `clearAbove`), whether every part traces a path back to
 * the floor or its wall through the parts it touches (`touching`,
 * `looseParts`), and whether every glowing part (a screen, a frame, a
 * portal, a signal light or a blinking one) touches a lit host or its
 * wall. A prop or hero mesh, built once at the origin, is turned and
 * placed the way the GPU places an instance (`placeMesh`, `placeParts`)
 * before it is measured. The glow check works on a list of recorded kit
 * calls (`Part`), which `recordingKitAt` records for a model built through
 * a kit factory, and which the fixture tests record for the movers too;
 * `heroAt`, `anchorOf` and `partsOf` are the one way every hero test
 * builds a hero and its recorded parts at the origin.
 */

import type { Box, Hero, HeroKind } from "../world/types";
import { CELL } from "../world/units";
import { BLINK_GROUPS } from "./blink";
import {
  FLAG,
  FLOATS_PER_VERTEX,
  createBuilder,
  type Builder,
  type MeshData,
  type Surface,
  type V3,
} from "./geometry";
import { createKit, turnPoint, type Frame, type Kit } from "./kit";
import { LOOKS } from "./looks";
import type { KitAt } from "./models";
import { buildHero } from "./models/heroes";

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
 * How close two parts must come to count as touching in the float check: a
 * little over `DECAL_LIFT`, so a light or decal on its face counts, and
 * well under `reaches`' 3 cm, so a 1.5 cm gap under a shelf or a pack does
 * not.
 */
const TOUCH = 0.012;

/**
 * Whether one shape is a sleeve round the other: their bounds overlap on
 * every axis, and on at least two axes one's range holds the other's whole
 * (a collar round a barrel, a ring round a tube). No vertex of either lies
 * near a triangle of the other, so the vertex test alone would call it
 * loose.
 */
function sleeve(p: Shape, q: Shape): boolean {
  const axes = [0, 1, 2] as const;
  if (!axes.every((k) => p.lo[k] < q.hi[k] && q.lo[k] < p.hi[k])) return false;
  const holds = (o: Shape, i: Shape) =>
    axes.filter((k) => o.lo[k] <= i.lo[k] + 1e-6 && i.hi[k] <= o.hi[k] + 1e-6)
      .length >= 2;
  return holds(p, q) || holds(q, p);
}

/**
 * Whether two shapes overlap in volume: their bounds overlap on every axis
 * by more than 1 cm. Catches a part built to run through another on
 * purpose (the laser desk's arm through the emitter housing) that neither
 * `sleeve` nor a vertex-to-triangle distance would call touching, without
 * loosening the check for a true floater, whose bounds miss every part it
 * should be resting on entirely.
 */
function overlapsVolume(p: Shape, q: Shape): boolean {
  const axes = [0, 1, 2] as const;
  return axes.every(
    (k) => Math.min(p.hi[k], q.hi[k]) - Math.max(p.lo[k], q.lo[k]) > 0.01,
  );
}

/**
 * Whether one shape sits directly on the other with no seam: one's lowest
 * point lands within 0.1 mm of the other's highest (either way round), and
 * their plan footprints (the other two axes) overlap by more than 1 cm on
 * both. Catches a part built to abut another exactly at the height a model
 * steps in or out (a dome planter's collar flaring past its drum, a rim
 * wider than the band it caps): the two never share a height, so `sleeve`
 * and `overlapsVolume` both call it loose, and the step can run wider than
 * `TOUCH`, so the vertex-to-triangle distance misses it too.
 */
function stacked(p: Shape, q: Shape): boolean {
  const flush = (a: Shape, b: Shape) => Math.abs(a.hi[1] - b.lo[1]) <= 1e-4;
  if (!flush(p, q) && !flush(q, p)) return false;
  const plan = [0, 2] as const;
  return plan.every(
    (k) => Math.min(p.hi[k], q.hi[k]) - Math.max(p.lo[k], q.lo[k]) > 0.01,
  );
}

/**
 * Whether two parts touch: one is a sleeve round the other (`sleeve`), they
 * overlap in volume by more than 1 cm on every axis (`overlapsVolume`), one
 * sits flush on the other with a footprint they share by more than 1 cm
 * (`stacked`), or a vertex of one lies within `TOUCH` of a triangle of the
 * other.
 */
export function touching(p: Shape, q: Shape): boolean {
  const axes = [0, 1, 2] as const;
  if (axes.some((k) => p.lo[k] > q.hi[k] + TOUCH || q.lo[k] > p.hi[k] + TOUCH))
    return false;
  if (sleeve(p, q) || overlapsVolume(p, q) || stacked(p, q)) return true;
  const near = (from: Shape, to: Shape) => {
    for (let t = 0; t + 2 < to.points.length; t += 3) {
      const [a, b, c] = [to.points[t], to.points[t + 1], to.points[t + 2]];
      if (!a || !b || !c) continue;
      for (const v of from.points)
        if (Math.hypot(...sub(v, closestOnTriangle(v, a, b, c))) <= TOUCH)
          return true;
    }
    return false;
  };
  return near(p, q) || near(q, p);
}

/**
 * Every part with no path back to the floor or the wall through the parts
 * it touches (`touching`): held parts start on the floor (a shape whose
 * lowest point sits within 0.1 mm of `y = 0`) or, when `wall` is given, on
 * the wall plane (some vertex within 0.1 mm of it, in the wall frame's
 * terms); every other part joins once it touches a held one, repeated to a
 * fixed point. What is left after that is loose, named `"<index>:<method>"`
 * in build order: a genuine floater, not a part chained to the floor only
 * through parts still unheld when it was its turn to check. Pass every
 * part of a hero, built once at the origin; a part with no points (an
 * empty primitive) is dropped rather than counted loose.
 */
export function looseParts(
  parts: readonly Part[],
  wall: Frame | null,
): string[] {
  const solid = parts.filter((p) => p.points.length > 0);
  const shapes = solid.map((p) => shape(p.points));
  const held = shapes.map(
    (s) =>
      s.lo[1] <= 1e-4 ||
      (wall !== null && s.points.some((q) => toLocal(wall, q)[1] <= 1e-4)),
  );
  for (let changed = true; changed;) {
    changed = false;
    shapes.forEach((s, i) => {
      if (held[i]) return;
      if (shapes.some((o, j) => held[j] && touching(s, o))) {
        held[i] = true;
        changed = true;
      }
    });
  }
  return solid
    .map((p, i) => ({ p, i }))
    .filter(({ i }) => !held[i])
    .map(({ p, i }) => `${String(i)}:${p.method}`);
}

/**
 * Whether some triangle of `mesh` faces straight up (its stored normal
 * within 1e-3 of `(0, 1, 0)`), has all three corners at height `h` within
 * 5 mm, and contains `(x, z)` in plan (the same half-plane sign on every
 * edge): a catalogue surface is real geometry, not just a number in the
 * catalogue.
 */
export function upwardFaceAt(
  mesh: MeshData,
  x: number,
  z: number,
  h: number,
): boolean {
  const ps = positions(mesh);
  const ns = normals(mesh);
  for (let t = 0; t + 2 < ps.length; t += 3) {
    const [a, b, c, n] = [ps[t], ps[t + 1], ps[t + 2], ns[t]];
    if (!a || !b || !c || !n) continue;
    if (Math.hypot(n[0], n[1] - 1, n[2]) > 1e-3) continue;
    if ([a, b, c].some((p) => Math.abs(p[1] - h) > 0.005)) continue;
    const side = (p: V3, q: V3) =>
      (q[0] - p[0]) * (z - p[2]) - (q[2] - p[2]) * (x - p[0]);
    const s = [side(a, b), side(b, c), side(c, a)];
    if (s.every((v) => v >= -1e-9) || s.every((v) => v <= 1e-9)) return true;
  }
  return false;
}

/**
 * Whether the column above `(x, z)`, from `h` up to `h + headroom` metres,
 * is free of mesh: no triangle whose plan footprint contains the point
 * rises above `h` (the surface's own top face, sitting at exactly `h`,
 * does not count) while staying below `h + headroom`. A part hovering in
 * reach over a catalogue surface fails this even where it never touches
 * the surface's own height.
 */
export function clearAbove(
  mesh: MeshData,
  x: number,
  z: number,
  h: number,
  headroom: number,
): boolean {
  const ps = positions(mesh);
  for (let t = 0; t + 2 < ps.length; t += 3) {
    const [a, b, c] = [ps[t], ps[t + 1], ps[t + 2]];
    if (!a || !b || !c) continue;
    const side = (p: V3, q: V3) =>
      (q[0] - p[0]) * (z - p[2]) - (q[2] - p[2]) * (x - p[0]);
    const s = [side(a, b), side(b, c), side(c, a)];
    if (!(s.every((v) => v >= -1e-9) || s.every((v) => v <= 1e-9))) continue;
    const lo = Math.min(a[1], b[1], c[1]);
    const hi = Math.max(a[1], b[1], c[1]);
    if (hi > h + 0.005 && lo < h + headroom) return false;
  }
  return true;
}

/** A free hero at turn 0, centred on the middle of a cell's width on a row line. */
export function heroAt(kind: HeroKind, variant = 0): Hero {
  return { kind, variant, x: 4.5, y: 3, turn: 0, seed: 1 };
}

/** Where a hero's mesh is placed, in world metres. */
export const anchorOf = (h: Hero): V3 => [h.x * CELL, 0, h.y * CELL];

/** A hero's recorded parts, built at the origin at turn 0. */
export function partsOf(kind: HeroKind, variant = 0): Part[] {
  const parts: Part[] = [];
  buildHero(
    recordingKitAt(createBuilder(), parts),
    kind,
    variant,
    LOOKS.aperture,
  );
  return parts;
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

/** Whether a flag is a signal light's or a blinking light's: what a frame may host. */
const isLight = (flag: number) =>
  flag === FLAG.signal ||
  (flag >= FLAG.blink && flag < FLAG.blink + BLINK_GROUPS);

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
 * surface and adds its edge lines), so a frame part hosts a signal or a
 * blinking light: a door's warning lamp sits on the frame of its jamb. It
 * hosts nothing else (a screen or a portal on a frame is still named), and
 * a frame itself still needs a lit host or the wall.
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
      const own = isLight(p.flag) ? [...hosts, ...frames] : hosts;
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
