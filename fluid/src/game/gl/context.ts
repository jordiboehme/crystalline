/**
 * A WebGL2 context and what it can do, found out once.
 *
 * The one capability that matters is whether a floating-point colour target
 * can be rendered to: bloom wants the scene in a half-float target so bright
 * neon keeps its energy past 1.0. `EXT_color_buffer_float` (or the
 * half-float variant on some Safari builds) says it should work, but a
 * driver can still refuse the framebuffer, so the claim is tested by
 * building a tiny target and asking whether it is complete. RGBA16F rather
 * than RGBA32F: it is filterable in core WebGL2, which the bloom's linear
 * taps rely on. Where it fails the renderer uses RGBA8 and a lower bloom
 * threshold. `?bloom=rgba8` forces that path for testing.
 */

import { createTarget } from "./target";

/**
 * The colour format of the offscreen targets: `rgba16f` where the driver
 * renders to half floats, `rgba8` everywhere else or when forced.
 */
export type ColorFormat = "rgba16f" | "rgba8";

/**
 * What the context can do, probed once at creation: the colour format the
 * render targets use and the most layers a texture array may hold, which
 * bounds how many surface and text layers the renderer can upload.
 */
export interface GlCaps {
  color: ColorFormat;
  maxLayers: number;
}

/**
 * Creates the game's WebGL2 context on `canvas` and probes its caps.
 *
 * No multisampling (the post passes work on offscreen targets anyway), no
 * alpha, a depth buffer and a request for the fast GPU. The half-float
 * path is taken only when an extension claims it and a 4x4 probe target
 * actually comes out complete; `forceRgba8` skips the probe. Returns null
 * when the browser gives no WebGL2 context at all.
 */
export function createContext(
  canvas: HTMLCanvasElement,
  options: { forceRgba8: boolean },
): { gl: WebGL2RenderingContext; caps: GlCaps } | null {
  const gl = canvas.getContext("webgl2", {
    antialias: false,
    alpha: false,
    depth: true,
    stencil: false,
    powerPreference: "high-performance",
    preserveDrawingBuffer: false,
  });
  if (gl === null) return null;
  let color: ColorFormat = "rgba8";
  if (
    !options.forceRgba8 &&
    (gl.getExtension("EXT_color_buffer_float") !== null ||
      gl.getExtension("EXT_color_buffer_half_float") !== null)
  ) {
    const probe = createTarget(gl, 4, 4, "rgba16f", false);
    if (probe !== null) {
      color = "rgba16f";
      probe.dispose();
    }
  }
  const maxLayers = gl.getParameter(gl.MAX_ARRAY_TEXTURE_LAYERS) as number;
  return { gl, caps: { color, maxLayers } };
}

/**
 * Whether this browser can make a WebGL2 context at all.
 *
 * Creates a throwaway canvas, asks it for a context and releases the
 * context straight away through `WEBGL_lose_context`, so the check does not
 * hold one of the browser's limited context slots. Never throws.
 */
export function hasWebGL2(): boolean {
  try {
    const canvas = document.createElement("canvas");
    const gl = canvas.getContext("webgl2");
    gl?.getExtension("WEBGL_lose_context")?.loseContext();
    return gl !== null;
  } catch {
    return false;
  }
}
