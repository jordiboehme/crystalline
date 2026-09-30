/**
 * The modelling kit: every detailed model in the station (terminals, desks,
 * lab benches, doors, tanks) is built in code from a handful of primitives,
 * with no model files and no loader.
 *
 * A kit is bound to a builder and a frame. A frame is a local coordinate
 * system on the floor: `origin` at floor level, `along` a horizontal unit
 * vector, `inward` the horizontal unit vector the model faces, and up is
 * always +y. Every primitive takes its sizes in that frame - `a` along,
 * `d` inward, `h` up - so a recipe is written once and stands on any wall
 * side or turns with any piece of decor. The frame's handedness is fixed:
 * `along x up = inward`, which makes `(a, h, d)` a right-handed system like
 * world `(x, y, z)`, so a face that is counter-clockwise in local terms is
 * counter-clockwise in the world too.
 *
 * Every primitive emits whole triangles into the builder, wound
 * counter-clockwise seen from outside, each face with its own flat unit
 * normal (the plane of the face). There is no smoothing: the `flat` look
 * needs none, and faceted normals read as detail under the banded light.
 * The uv is in metres, starting at 0 on a face's first corner, so the
 * shader's edge lines outline each flat face; round surfaces carry one
 * continuous uv around their circumference instead, so a cylinder is not
 * striped with a line per facet.
 *
 * The kit test checks every primitive in all four wall frames and in turned
 * decor frames: winding against the stored normal, the declared bounds, a
 * pinned vertex count, and a positive enclosed volume for closed shapes.
 */

import { CELL } from "../world/units";
import type { Decor, WallSlot } from "../world/types";
import type { Builder, Surface, V3 } from "./geometry";

/**
 * A local coordinate system for a model: `origin` on the floor, `along`
 * and `inward` horizontal unit vectors with `along x up = inward`. A wall
 * frame's `inward` points into the room and a decor frame's points where
 * the piece faces.
 */
export interface Frame {
  origin: V3;
  along: V3;
  inward: V3;
}

/**
 * How far, in metres, a decal stands off the face it is laid on: every
 * `panel` that shows text, a pictogram, a screen or a hazard stripe on top
 * of a body, a plate or the wall, and every text panel of `textPanel`.
 *
 * The station draws with a near plane of 0.02 m into a 24-bit depth
 * buffer, so the depth buffer's steps grow with the square of the
 * distance: about 1 mm at 18 m, 10 mm at 58 m. A 1 mm lift lost that race
 * from about 18 m on and the text flickered through its backing across a
 * hub's hall; 10 mm holds to about 58 m, where a poster is a dozen pixels
 * tall. It is also the one number the models test measures every decal
 * against, so a new flush prop that forgets it fails there rather than on
 * screen.
 */
export const DECAL_LIFT = 0.01;

/** A point in a frame's local terms: `[a, d, h]`. */
type Local = readonly [a: number, d: number, h: number];

/** A uv pair in metres (or in whole-layer units for a panel). */
type Uv = readonly [u: number, v: number];

/**
 * The primitives, all emitting into the kit's builder in the kit's frame.
 * Ranges may be given in either order; each primitive sorts its own. Sizes
 * are metres. `sides` and `segments` are facet counts around a circle.
 */
export interface Kit {
  /**
   * A plain box from `a0..a1`, `d0..d1`, `h0..h1`, six faces facing out:
   * slabs, legs, shelves, keyboards and anything square. 36 vertices.
   */
  box(
    a0: number,
    a1: number,
    d0: number,
    d1: number,
    h0: number,
    h1: number,
    s: Surface,
  ): void;
  /**
   * A box with every one of its twelve edges chamfered by `bevel`: 6 inset
   * faces, 12 edge strips and 8 corner triangles, 26 faces and 132
   * vertices. For housings, desks, frames and cabinets, where a bevel
   * catches the light and reads as machined metal. The bevel is clamped to
   * 45 percent of the smallest extent so no face collapses; a bevel of 0 or
   * less gives a plain `box`.
   */
  bevelBox(
    a0: number,
    a1: number,
    d0: number,
    d1: number,
    h0: number,
    h1: number,
    bevel: number,
    s: Surface,
  ): void;
  /**
   * An upright cylinder of `sides` flat facets around the vertical axis at
   * `(a, d)`, from `h0` to `h1`: chair bases, posts, tanks, columns. With
   * `caps` (the default) a fan closes each end: `sides * 6` vertices for
   * the side and `sides * 6` for the two caps; without, `sides * 6`.
   */
  cylinder(
    a: number,
    d: number,
    h0: number,
    h1: number,
    radius: number,
    sides: number,
    s: Surface,
    caps?: boolean,
  ): void;
  /**
   * A capped cylinder lying along the wall, its axis at depth `d` and
   * height `h` from `a0` to `a1`: pipes, rails, rollers and the hub of a
   * handwheel. `sides * 12` vertices.
   */
  cylinderAlong(
    a0: number,
    a1: number,
    d: number,
    h: number,
    radius: number,
    sides: number,
    s: Surface,
  ): void;
  /**
   * A torus centred at `(a, d, h)`: `radius` to the middle of the tube,
   * `tube` the tube's radius, `sides` facets around the tube and `segments`
   * around the ring. `"inward"` stands it in the wall plane (handwheels,
   * portal rings, porthole rims); `"up"` lays it flat (a table rim, a
   * collar on a tank). `segments * sides * 6` vertices.
   */
  ring(
    a: number,
    d: number,
    h: number,
    radius: number,
    tube: number,
    sides: number,
    segments: number,
    s: Surface,
    facing: "inward" | "up",
  ): void;
  /**
   * A profile of `[radius, height]` points revolved around the vertical
   * axis at `(a, d)`: flasks, specimen tanks, pods, lamp shades, domes.
   * The side of the profile on the right of its direction of travel (with
   * radius to the right and height up) faces out. So an open lathe, such
   * as a vase wall or a lamp shade, must run from the bottom up to face
   * outward; run it from the top down to get the inside of a bowl. Start
   * and end the profile at radius 0 to close the ends, walking out along
   * the bottom, up the side and back in across the top. Each segment gives `sides * 6` vertices, or
   * `sides * 3` where one end sits on the axis; a segment of no length or
   * lying on the axis gives none.
   */
  lathe(
    a: number,
    d: number,
    profile: readonly (readonly [r: number, h: number])[],
    sides: number,
    s: Surface,
  ): void;
  /**
   * An outline of `[a, h]` points in the wall plane, extruded out from the
   * wall between depths `d0` and `d1`: console bodies with a sloped deck,
   * door frames, brackets, signs. The outline must be simple (it may be
   * concave) and may run either way round; repeated and collinear points
   * are dropped, and an outline whose edges cross or that encloses no area
   * throws. With `n` points left: a front and a back face of `n - 2`
   * triangles each and `n` side quads, `12 * n - 12` vertices.
   */
  extrude(
    polygon: readonly (readonly [a: number, h: number])[],
    d0: number,
    d1: number,
    s: Surface,
  ): void;
  /**
   * One flat quad at depth `d` facing inward, with a uv rectangle from
   * `(u0, v0)` at its bottom-left corner to `(u0 + uw, v0 + vh)` at its
   * top-right (default the whole layer, 0 to 1 both ways): screens,
   * labels, posters and the text layers, as milestone 1's wall panel. The
   * offsets let a label use one row of a shared text layer (its `v0` to
   * `v1`) or one pictogram of a sheet. 6 vertices.
   */
  panel(
    a0: number,
    a1: number,
    d: number,
    h0: number,
    h1: number,
    s: Surface,
    uw?: number,
    vh?: number,
    u0?: number,
    v0?: number,
  ): void;
}

/** Points closer than this are the same point. */
const SAME = 1e-9;

const minus = (p: readonly number[], q: readonly number[]): V3 => [
  (p[0] ?? 0) - (q[0] ?? 0),
  (p[1] ?? 0) - (q[1] ?? 0),
  (p[2] ?? 0) - (q[2] ?? 0),
];
const crossV = (p: V3, q: V3): V3 => [
  p[1] * q[2] - p[2] * q[1],
  p[2] * q[0] - p[0] * q[2],
  p[0] * q[1] - p[1] * q[0],
];
const dotV = (p: V3, q: V3) => p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
const lengthV = (p: V3) => Math.hypot(p[0], p[1], p[2]);
const scaleV = (p: V3, k: number): V3 => [p[0] * k, p[1] * k, p[2] * k];

/**
 * A polygon's normal by Newell's method, measured from its first point so
 * large world offsets cost no precision. Its length is twice the area.
 */
function newell(pts: readonly V3[]): V3 {
  const first = pts[0];
  if (!first) return [0, 0, 0];
  const n: V3 = [0, 0, 0];
  for (let i = 0; i < pts.length; i++) {
    const p = minus(pts[i] ?? first, first);
    const q = minus(pts[(i + 1) % pts.length] ?? first, first);
    n[0] += (p[1] - q[1]) * (p[2] + q[2]);
    n[1] += (p[2] - q[2]) * (p[0] + q[0]);
    n[2] += (p[0] - q[0]) * (p[1] + q[1]);
  }
  return n;
}

/** Twice the signed area of the triangle `p q r` in a plane, positive counter-clockwise. */
const cross2 = (
  p: readonly [number, number],
  q: readonly [number, number],
  r: readonly [number, number],
) => (q[0] - p[0]) * (r[1] - p[1]) - (r[0] - p[0]) * (q[1] - p[1]);

/** Whether `r` lies on the segment `p q`, given that the three are collinear. */
const within = (
  p: readonly [number, number],
  q: readonly [number, number],
  r: readonly [number, number],
) =>
  r[0] >= Math.min(p[0], q[0]) - SAME &&
  r[0] <= Math.max(p[0], q[0]) + SAME &&
  r[1] >= Math.min(p[1], q[1]) - SAME &&
  r[1] <= Math.max(p[1], q[1]) + SAME;

/** Whether the segments `p q` and `r t` cross or touch. */
function segmentsMeet(
  p: readonly [number, number],
  q: readonly [number, number],
  r: readonly [number, number],
  t: readonly [number, number],
): boolean {
  const o1 = cross2(p, q, r);
  const o2 = cross2(p, q, t);
  const o3 = cross2(r, t, p);
  const o4 = cross2(r, t, q);
  const apart = (x: number, y: number) =>
    (x > SAME && y < -SAME) || (x < -SAME && y > SAME);
  if (apart(o1, o2) && apart(o3, o4)) return true;
  return (
    (Math.abs(o1) <= SAME && within(p, q, r)) ||
    (Math.abs(o2) <= SAME && within(p, q, t)) ||
    (Math.abs(o3) <= SAME && within(r, t, p)) ||
    (Math.abs(o4) <= SAME && within(r, t, q))
  );
}

/**
 * A plane outline made clean for extrusion: repeated and collinear points
 * dropped, then turned counter-clockwise. Throws when two edges that are
 * not neighbours cross or touch (a bowtie, a figure eight) or when what is
 * left encloses no area, since recipes are code and a broken one should
 * fail loudly rather than emit garbage.
 */
function cleanOutline(
  polygon: readonly (readonly [number, number])[],
): [number, number][] {
  let pts = polygon.map(([x, y]) => [x, y] as [number, number]);
  for (let changed = true; changed && pts.length >= 3;) {
    changed = false;
    for (let i = 0; i < pts.length; i++) {
      const p = pts[(i + pts.length - 1) % pts.length];
      const q = pts[i];
      const r = pts[(i + 1) % pts.length];
      if (!p || !q || !r) continue;
      const repeated = Math.hypot(q[0] - r[0], q[1] - r[1]) < SAME;
      if (repeated || Math.abs(cross2(p, q, r)) < SAME) {
        pts.splice(i, 1);
        changed = true;
        break;
      }
    }
  }
  const n = pts.length;
  for (let i = 0; i < n; i++) {
    for (let j = i + 2; j < n; j++) {
      if (i === 0 && j === n - 1) continue;
      const [p, q] = [pts[i], pts[(i + 1) % n]];
      const [r, t] = [pts[j], pts[(j + 1) % n]];
      if (p && q && r && t && segmentsMeet(p, q, r, t))
        throw new Error("extrude: the outline is not a simple polygon");
    }
  }
  let area = 0;
  pts.forEach((p, i) => {
    const q = pts[(i + 1) % pts.length] ?? p;
    area += p[0] * q[1] - q[0] * p[1];
  });
  if (pts.length < 3 || Math.abs(area) / 2 < SAME)
    throw new Error("extrude: the outline encloses no area");
  if (area < 0) pts = pts.reverse();
  return pts;
}

/**
 * Triangulates a simple counter-clockwise outline by ear clipping: a
 * convex corner whose triangle holds no other point of the outline is cut
 * off, until three points remain. Returns `n - 2` counter-clockwise index
 * triples. `cleanOutline` has already rejected crossing edges; the throw
 * here is a last guard should clipping still stall.
 */
function earClip(pts: readonly [number, number][]): [number, number, number][] {
  const idx = pts.map((_, i) => i);
  const out: [number, number, number][] = [];
  const at = (i: number) => pts[i] ?? [NaN, NaN];
  while (idx.length > 3) {
    let cut = false;
    for (let k = 0; k < idx.length && !cut; k++) {
      const i0 = idx[(k + idx.length - 1) % idx.length] ?? 0;
      const i1 = idx[k] ?? 0;
      const i2 = idx[(k + 1) % idx.length] ?? 0;
      const [p0, p1, p2] = [at(i0), at(i1), at(i2)];
      if (cross2(p0, p1, p2) <= SAME) continue;
      const holds = idx.some((j) => {
        if (j === i0 || j === i1 || j === i2) return false;
        const p = at(j);
        return (
          cross2(p0, p1, p) >= -SAME &&
          cross2(p1, p2, p) >= -SAME &&
          cross2(p2, p0, p) >= -SAME
        );
      });
      if (holds) continue;
      out.push([i0, i1, i2]);
      idx.splice(k, 1);
      cut = true;
    }
    if (!cut) throw new Error("extrude: the outline is not a simple polygon");
  }
  const [i0 = 0, i1 = 0, i2 = 0] = idx;
  out.push([i0, i1, i2]);
  return out;
}

/** The axis a revolved surface turns around, in local terms. */
type Axis = "a" | "d" | "h";

/**
 * Binds the modelling kit to a builder and a frame: every primitive called
 * on the returned kit emits its triangles into `builder`, placed by
 * `frame`. Make one kit per model (or per wall slot) and throw it away.
 */
export function createKit(builder: Builder, frame: Frame): Kit {
  const { origin, along, inward } = frame;

  const world = ([a, d, h]: Local): V3 => [
    origin[0] + along[0] * a + inward[0] * d,
    origin[1] + along[1] * a + inward[1] * d + h,
    origin[2] + along[2] * a + inward[2] * d,
  ];

  /**
   * Emits one flat face from its corners in order, counter-clockwise seen
   * from outside. Repeated corners are dropped (a quad touching an axis
   * becomes a triangle) and a face with no area emits nothing. `tris`
   * triangulates a concave face; a convex one is fanned from its first
   * corner. Without `uvs`, the uv is the face's own plane in metres, `u`
   * along its first edge.
   */
  const face = (
    corners: readonly Local[],
    s: Surface,
    uvs?: readonly Uv[],
    tris?: readonly (readonly [number, number, number])[],
  ) => {
    const pts: V3[] = [];
    const uv: Uv[] = [];
    corners.forEach((c, i) => {
      const p = world(c);
      const prev = pts[pts.length - 1];
      if (prev && lengthV(minus(p, prev)) < SAME) return;
      pts.push(p);
      uv.push(uvs?.[i] ?? [0, 0]);
    });
    const first = pts[0];
    const last = pts[pts.length - 1];
    if (!tris && first && last && pts.length > 1) {
      if (lengthV(minus(first, last)) < SAME) {
        pts.pop();
        uv.pop();
      }
    }
    if (pts.length < 3 || !first) return;
    const raw = newell(pts);
    const len = lengthV(raw);
    if (len < SAME) return;
    const n = scaleV(raw, 1 / len);
    if (!uvs) {
      const second = pts[1] ?? first;
      const edge = minus(second, first);
      const u = scaleV(edge, 1 / lengthV(edge));
      const v = crossV(n, u);
      pts.forEach((p, i) => {
        const o = minus(p, first);
        uv[i] = [dotV(o, u), dotV(o, v)];
      });
    }
    const emit = (i: number) => {
      const p = pts[i] ?? first;
      const [u, v] = uv[i] ?? [0, 0];
      builder.vertex(p, n, u, v, s);
    };
    const order =
      tris ??
      Array.from(
        { length: pts.length - 2 },
        (_, i) => [0, i + 1, i + 2] as const,
      );
    for (const [i, j, k] of order) {
      emit(i);
      emit(j);
      emit(k);
    }
  };

  /**
   * Revolves a profile of `[r, t]` points around `axis` through `centre`:
   * `t` runs along the axis from the centre and `r` away from it. The side
   * of the profile to the right of its direction of travel (radius right,
   * axial distance up) faces out. The radial basis `e1`, `e2` is chosen so
   * `e1 x axis = e2` in the right-handed `(a, h, d)` system, which keeps
   * each quad counter-clockwise seen from outside.
   */
  const revolve = (
    centre: Local,
    axis: Axis,
    profile: readonly (readonly [r: number, t: number])[],
    sides: number,
    s: Surface,
  ) => {
    const n = Math.max(3, Math.round(sides));
    const at = (r: number, t: number, i: number): Local => {
      const angle = (2 * Math.PI * (i % n)) / n;
      const x = r * Math.sin(angle);
      const y = r * Math.cos(angle);
      const [ca, cd, ch] = centre;
      switch (axis) {
        case "h": // e1 = a, e2 = d
          return [ca + x, cd + y, ch + t];
        case "a": // e1 = d, e2 = h
          return [ca + t, cd + x, ch + y];
        case "d": // e1 = h, e2 = a
          return [ca + y, cd + t, ch + x];
      }
    };
    let v0 = 0;
    for (let k = 0; k + 1 < profile.length; k++) {
      const [r0 = 0, t0 = 0] = profile[k] ?? [];
      const [r1 = 0, t1 = 0] = profile[k + 1] ?? [];
      const v1 = v0 + Math.hypot(r1 - r0, t1 - t0);
      for (let i = 0; i < n; i++) {
        const u0 = (2 * Math.PI * i) / n;
        const u1 = (2 * Math.PI * (i + 1)) / n;
        face(
          [at(r0, t0, i), at(r0, t0, i + 1), at(r1, t1, i + 1), at(r1, t1, i)],
          s,
          [
            [r0 * u0, v0],
            [r0 * u1, v0],
            [r1 * u1, v1],
            [r1 * u0, v1],
          ],
        );
      }
      v0 = v1;
    }
  };

  const box: Kit["box"] = (a0, a1, d0, d1, h0, h1, s) => {
    const [A0, A1] = [Math.min(a0, a1), Math.max(a0, a1)];
    const [D0, D1] = [Math.min(d0, d1), Math.max(d0, d1)];
    const [H0, H1] = [Math.min(h0, h1), Math.max(h0, h1)];
    // Inward, outward (against the wall), right, left, top, bottom.
    face(
      [
        [A0, D1, H0],
        [A1, D1, H0],
        [A1, D1, H1],
        [A0, D1, H1],
      ],
      s,
    );
    face(
      [
        [A1, D0, H0],
        [A0, D0, H0],
        [A0, D0, H1],
        [A1, D0, H1],
      ],
      s,
    );
    face(
      [
        [A1, D1, H0],
        [A1, D0, H0],
        [A1, D0, H1],
        [A1, D1, H1],
      ],
      s,
    );
    face(
      [
        [A0, D0, H0],
        [A0, D1, H0],
        [A0, D1, H1],
        [A0, D0, H1],
      ],
      s,
    );
    face(
      [
        [A0, D1, H1],
        [A1, D1, H1],
        [A1, D0, H1],
        [A0, D0, H1],
      ],
      s,
    );
    face(
      [
        [A0, D0, H0],
        [A1, D0, H0],
        [A1, D1, H0],
        [A0, D1, H0],
      ],
      s,
    );
  };

  return {
    box,

    bevelBox(a0, a1, d0, d1, h0, h1, bevel, s) {
      const [A0, A1] = [Math.min(a0, a1), Math.max(a0, a1)];
      const [D0, D1] = [Math.min(d0, d1), Math.max(d0, d1)];
      const [H0, H1] = [Math.min(h0, h1), Math.max(h0, h1)];
      const b = Math.min(bevel, 0.45 * Math.min(A1 - A0, D1 - D0, H1 - H0));
      if (!(b > 0)) {
        box(A0, A1, D0, D1, H0, H1, s);
        return;
      }
      // The outline of the vertical sides with their corners cut, going
      // round the same way as the box's side faces.
      const ring: [number, number][] = [
        [A0 + b, D1],
        [A1 - b, D1],
        [A1, D1 - b],
        [A1, D0 + b],
        [A1 - b, D0],
        [A0 + b, D0],
        [A0, D0 + b],
        [A0, D1 - b],
      ];
      // Each outline point pulled in onto the top and bottom faces.
      const inset = ([a, d]: [number, number]): [number, number] => [
        Math.min(Math.max(a, A0 + b), A1 - b),
        Math.min(Math.max(d, D0 + b), D1 - b),
      ];
      const [Hb, Ht] = [H0 + b, H1 - b];
      ring.forEach((o0, i) => {
        const o1 = ring[(i + 1) % ring.length] ?? o0;
        const [t0, t1] = [inset(o0), inset(o1)];
        // The side (or vertical edge strip), the top strip or corner, the
        // bottom strip or corner; a corner is a quad whose top points
        // coincide, which `face` turns into a triangle.
        face(
          [
            [o0[0], o0[1], Hb],
            [o1[0], o1[1], Hb],
            [o1[0], o1[1], Ht],
            [o0[0], o0[1], Ht],
          ],
          s,
        );
        face(
          [
            [o0[0], o0[1], Ht],
            [o1[0], o1[1], Ht],
            [t1[0], t1[1], H1],
            [t0[0], t0[1], H1],
          ],
          s,
        );
        face(
          [
            [t0[0], t0[1], H0],
            [t1[0], t1[1], H0],
            [o1[0], o1[1], Hb],
            [o0[0], o0[1], Hb],
          ],
          s,
        );
      });
      face(
        [
          [A0 + b, D1 - b, H1],
          [A1 - b, D1 - b, H1],
          [A1 - b, D0 + b, H1],
          [A0 + b, D0 + b, H1],
        ],
        s,
      );
      face(
        [
          [A0 + b, D0 + b, H0],
          [A1 - b, D0 + b, H0],
          [A1 - b, D1 - b, H0],
          [A0 + b, D1 - b, H0],
        ],
        s,
      );
    },

    cylinder(a, d, h0, h1, radius, sides, s, caps = true) {
      const [H0, H1] = [Math.min(h0, h1), Math.max(h0, h1)];
      const r = Math.abs(radius);
      revolve(
        [a, d, 0],
        "h",
        caps
          ? [
              [0, H0],
              [r, H0],
              [r, H1],
              [0, H1],
            ]
          : [
              [r, H0],
              [r, H1],
            ],
        sides,
        s,
      );
    },

    cylinderAlong(a0, a1, d, h, radius, sides, s) {
      const [A0, A1] = [Math.min(a0, a1), Math.max(a0, a1)];
      const r = Math.abs(radius);
      revolve(
        [0, d, h],
        "a",
        [
          [0, A0],
          [r, A0],
          [r, A1],
          [0, A1],
        ],
        sides,
        s,
      );
    },

    ring(a, d, h, radius, tube, sides, segments, s, facing) {
      const n = Math.max(3, Math.round(sides));
      // The tube's cross-section, counter-clockwise in (r, t) so its
      // outside is on the right of the direction of travel.
      const circle = Array.from({ length: n + 1 }, (_, j) => {
        const angle = (2 * Math.PI * (j % n)) / n;
        return [
          radius + tube * Math.cos(angle),
          tube * Math.sin(angle),
        ] as const;
      });
      revolve([a, d, h], facing === "up" ? "h" : "d", circle, segments, s);
    },

    lathe(a, d, profile, sides, s) {
      revolve([a, d, 0], "h", profile, sides, s);
    },

    extrude(polygon, d0, d1, s) {
      const [D0, D1] = [Math.min(d0, d1), Math.max(d0, d1)];
      const pts = cleanOutline(polygon);
      const tris = earClip(pts);
      const aMin = Math.min(...pts.map((p) => p[0]));
      const aMax = Math.max(...pts.map((p) => p[0]));
      const hMin = Math.min(...pts.map((p) => p[1]));
      // The front, read from the room; the back, read from behind.
      face(
        pts.map(([pa, ph]) => [pa, D1, ph] as const),
        s,
        pts.map(([pa, ph]) => [pa - aMin, ph - hMin] as const),
        tris,
      );
      // The back runs the outline the other way round, so its plane faces
      // the wall, and every triangle turns over with it.
      const back = [...pts].reverse();
      const m = pts.length - 1;
      face(
        back.map(([pa, ph]) => [pa, D0, ph] as const),
        s,
        back.map(([pa, ph]) => [aMax - pa, ph - hMin] as const),
        tris.map(([i, j, k]) => [m - i, m - k, m - j] as const),
      );
      pts.forEach((p, i) => {
        const q = pts[(i + 1) % pts.length] ?? p;
        face(
          [
            [p[0], D0, p[1]],
            [q[0], D0, q[1]],
            [q[0], D1, q[1]],
            [p[0], D1, p[1]],
          ],
          s,
        );
      });
    },

    panel(a0, a1, d, h0, h1, s, uw = 1, vh = 1, u0 = 0, v0 = 0) {
      // Bottom-left, bottom-right, top-right, top-left, as `builder.quad`
      // winds them, with the uv rectangle moved to start at (u0, v0).
      const corners = [
        [world([a0, d, h0]), u0, v0],
        [world([a1, d, h0]), u0 + uw, v0],
        [world([a1, d, h1]), u0 + uw, v0 + vh],
        [world([a0, d, h1]), u0, v0 + vh],
      ] as const;
      for (const i of [0, 1, 2, 0, 2, 3]) {
        const c = corners[i];
        if (c) builder.vertex(c[0], inward, c[1], c[2], s);
      }
    },
  };
}

/**
 * The frame for a wall slot (milestone 1's wall frame): `origin` on the
 * wall at floor level in the middle of the cell, `inward` into the room and
 * `along` so that `along x up = inward`. That handedness is what makes a
 * panel built bottom-left, bottom-right, top-right, top-left in `a` and `h`
 * wind counter-clockwise seen from the room.
 */
export function frameForSlot(slot: WallSlot): Frame {
  const cx = (slot.x + 0.5) * CELL;
  const cz = (slot.y + 0.5) * CELL;
  switch (slot.side) {
    case "n":
      return {
        origin: [cx, 0, slot.y * CELL],
        along: [1, 0, 0],
        inward: [0, 0, 1],
      };
    case "s":
      return {
        origin: [cx, 0, (slot.y + 1) * CELL],
        along: [-1, 0, 0],
        inward: [0, 0, -1],
      };
    case "w":
      return {
        origin: [slot.x * CELL, 0, cz],
        along: [0, 0, -1],
        inward: [1, 0, 0],
      };
    case "e":
      return {
        origin: [(slot.x + 1) * CELL, 0, cz],
        along: [0, 0, 1],
        inward: [-1, 0, 0],
      };
  }
}

/**
 * The facing (`inward`) and `along` of each quarter turn, clockwise seen
 * from above starting at north: north, east, south, west. A piece facing
 * north stands like a fixture on a south wall, so its `along` runs west.
 */
const TURNS: readonly (readonly [along: V3, inward: V3])[] = [
  [
    [-1, 0, 0],
    [0, 0, -1],
  ],
  [
    [0, 0, -1],
    [1, 0, 0],
  ],
  [
    [1, 0, 0],
    [0, 0, 1],
  ],
  [
    [0, 0, 1],
    [-1, 0, 0],
  ],
];

/**
 * A frame at a world point, facing north at `turn` 0 and a quarter turn
 * further clockwise (seen from above) per step, the convention of a
 * `Decor`'s `turn`. The turn is rounded to whole quarters and taken modulo
 * four, so any integer works.
 */
export function frameAt(origin: V3, turn: number): Frame {
  const q = ((Math.round(turn) % 4) + 4) % 4;
  const [along, inward] = TURNS[q] ?? TURNS[0] ?? [];
  if (!along || !inward) throw new Error("no turn table");
  return {
    origin: [origin[0], origin[1], origin[2]],
    along: [along[0], along[1], along[2]],
    inward: [inward[0], inward[1], inward[2]],
  };
}

/**
 * The quarter turns as 2x2 maps of (x, z), clockwise seen from above:
 * turn q sends (x, z) to (m[0] * x + m[1] * z, m[2] * x + m[3] * z).
 * This is the one rotation both the tests and the scene shader use (the
 * shader's table is emitted from it by `turnMat2Columns`); the kit test
 * pins it against `frameAt`'s TURNS table.
 *
 * A prop mesh is built once, in `frameAt([0, 0, 0], 0)`, and every
 * instance is placed by turning it with this table and adding its anchor.
 * Turn q of a point built at turn 0 lands exactly where `frameAt` at turn q
 * would have built it, so a recipe never needs to know how it is turned.
 */
export const TURN_XZ: readonly (readonly [number, number, number, number])[] = [
  [1, 0, 0, 1],
  [0, -1, 1, 0],
  [-1, 0, 0, -1],
  [0, 1, -1, 0],
];

/**
 * A point or direction turned by whole quarter turns about the vertical,
 * by `TURN_XZ`. The turn is rounded to whole quarters and taken modulo
 * four, as `frameAt` takes it; the height is left alone.
 */
export function turnPoint(p: V3, turn: number): V3 {
  const q = ((Math.round(turn) % 4) + 4) % 4;
  const m = TURN_XZ[q] ?? [1, 0, 0, 1];
  return [m[0] * p[0] + m[1] * p[2], p[1], m[2] * p[0] + m[3] * p[2]];
}

/**
 * A turn as the four numbers of a GLSL `mat2` constructor, which takes its
 * columns first: `mat2(c0.x, c0.y, c1.x, c1.y)`, so that `M * vec2(x, z)`
 * equals `turnPoint`.
 */
export function turnMat2Columns(
  turn: number,
): [number, number, number, number] {
  const q = ((Math.round(turn) % 4) + 4) % 4;
  const m = TURN_XZ[q] ?? [1, 0, 0, 1];
  return [m[0], m[2], m[1], m[3]];
}

/**
 * The frame for a piece of decor: origin at its centre on the floor
 * (`x` and `y` are continuous cell units), turned by its quarter turns. A
 * decor recipe is written centred on `a = 0, d = 0` with its front
 * towards `+d`.
 */
export function frameForDecor(decor: Decor): Frame {
  return frameAt([decor.x * CELL, 0, decor.y * CELL], decor.turn);
}
