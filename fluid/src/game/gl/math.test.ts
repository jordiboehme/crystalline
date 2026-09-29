import { describe, expect, it } from "vitest";

import {
  forwardOf,
  fpsView,
  mat4,
  multiply,
  perspective,
  rightOf,
  transformPoint,
} from "./math";

function close(actual: number, expected: number) {
  expect(actual).toBeCloseTo(expected, 5);
}

describe("fpsView", () => {
  it("looks north at yaw 0", () => {
    const view = fpsView(mat4(), [0, 0, 0], 0, 0);
    const [x, y, z] = transformPoint(view, [0, 0, -5]);
    close(x, 0);
    close(y, 0);
    close(z, -5);
  });

  it("turns left with a positive yaw", () => {
    const view = fpsView(mat4(), [0, 0, 0], Math.PI / 2, 0);
    const [x, , z] = transformPoint(view, [-5, 0, 0]);
    close(x, 0);
    close(z, -5);
  });

  it("moves the world opposite to the eye", () => {
    const view = fpsView(mat4(), [3, 1.6, 4], 0, 0);
    const [x, y, z] = transformPoint(view, [3, 1.6, -1]);
    close(x, 0);
    close(y, 0);
    close(z, -5);
  });

  it("tilts up with a positive pitch", () => {
    const view = fpsView(mat4(), [0, 0, 0], 0, Math.PI / 6);
    const [, y] = transformPoint(view, [0, 0, -5]);
    expect(y).toBeLessThan(0);
  });
});

describe("perspective", () => {
  it("maps the near plane to -1 and the far plane to +1 in depth", () => {
    const p = perspective(mat4(), Math.PI / 2, 1, 0.1, 100);
    const [, , zn, wn] = transformPoint(p, [0, 0, -0.1]);
    const [, , zf, wf] = transformPoint(p, [0, 0, -100]);
    close(zn / wn, -1);
    close(zf / wf, 1);
  });
});

describe("multiply", () => {
  it("treats the identity as neutral", () => {
    const p = perspective(mat4(), 1, 1.5, 0.1, 50);
    const out = multiply(mat4(), p, mat4());
    expect(Array.from(out)).toEqual(Array.from(p));
  });

  it("applies the right operand first", () => {
    const view = fpsView(mat4(), [0, 0, 5], 0, 0);
    const proj = perspective(mat4(), Math.PI / 2, 1, 0.1, 100);
    const both = multiply(mat4(), proj, view);
    const direct = transformPoint(both, [0, 0, 0]);
    const viewed = transformPoint(view, [0, 0, 0]);
    const staged = transformPoint(proj, [viewed[0], viewed[1], viewed[2]]);
    direct.forEach((v, i) => close(v, staged[i] ?? NaN));
  });
});

describe("forwardOf and rightOf", () => {
  it("agree with the view at yaw 0", () => {
    expect(forwardOf(0)).toEqual([-0, -1]);
    const [rx, rz] = rightOf(0);
    close(rx, 1);
    close(rz, 0);
  });
});
