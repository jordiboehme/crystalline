/**
 * An offscreen render target: a framebuffer with one colour texture and,
 * for the scene pass, a depth buffer. Linear filtering and clamped edges,
 * because every target is later sampled by a blur or a composite.
 *
 * Returns null instead of throwing when the driver calls the framebuffer
 * incomplete, so the caller can fall back to RGBA8.
 */

import type { ColorFormat } from "./context";

/**
 * A complete framebuffer and the texture it renders into. `width` and
 * `height` are the whole-pixel size actually allocated, which the caller
 * passes to `viewport` before drawing into it. `dispose` frees the
 * framebuffer, the texture and the depth buffer if there is one.
 */
export interface Target {
  framebuffer: WebGLFramebuffer;
  texture: WebGLTexture;
  width: number;
  height: number;
  dispose(): void;
}

/**
 * Builds a render target of `width` by `height` pixels (floored, at least
 * one) in `format`, with a 24-bit depth renderbuffer when `depth` is set.
 * Leaves no framebuffer or texture bound. Returns null, with everything it
 * allocated already freed, when the framebuffer is incomplete.
 */
export function createTarget(
  gl: WebGL2RenderingContext,
  width: number,
  height: number,
  format: ColorFormat,
  depth: boolean,
): Target | null {
  const w = Math.max(1, Math.floor(width));
  const h = Math.max(1, Math.floor(height));
  const texture = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, texture);
  if (format === "rgba16f") {
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA16F,
      w,
      h,
      0,
      gl.RGBA,
      gl.HALF_FLOAT,
      null,
    );
  } else {
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA8,
      w,
      h,
      0,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      null,
    );
  }
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

  const framebuffer = gl.createFramebuffer();
  gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
  gl.framebufferTexture2D(
    gl.FRAMEBUFFER,
    gl.COLOR_ATTACHMENT0,
    gl.TEXTURE_2D,
    texture,
    0,
  );
  let renderbuffer: WebGLRenderbuffer | null = null;
  if (depth) {
    renderbuffer = gl.createRenderbuffer();
    gl.bindRenderbuffer(gl.RENDERBUFFER, renderbuffer);
    gl.renderbufferStorage(gl.RENDERBUFFER, gl.DEPTH_COMPONENT24, w, h);
    gl.framebufferRenderbuffer(
      gl.FRAMEBUFFER,
      gl.DEPTH_ATTACHMENT,
      gl.RENDERBUFFER,
      renderbuffer,
    );
    gl.bindRenderbuffer(gl.RENDERBUFFER, null);
  }
  const complete =
    gl.checkFramebufferStatus(gl.FRAMEBUFFER) === gl.FRAMEBUFFER_COMPLETE;
  gl.bindFramebuffer(gl.FRAMEBUFFER, null);
  gl.bindTexture(gl.TEXTURE_2D, null);
  const dispose = () => {
    gl.deleteFramebuffer(framebuffer);
    gl.deleteTexture(texture);
    if (renderbuffer !== null) gl.deleteRenderbuffer(renderbuffer);
  };
  if (!complete) {
    dispose();
    return null;
  }
  return { framebuffer, texture, width: w, height: h, dispose };
}
