/**
 * The one `TEXTURE_2D_ARRAY` every surface samples from.
 *
 * Storage is allocated once for all layers with `texStorage3D`, each layer
 * is filled with `texSubImage3D`, and `finish` builds the mipmaps, so
 * distant panels do not shimmer. A single text layer can be replaced later
 * (a terminal that changes), followed by another `finish`.
 */

/**
 * The texture array and its upload calls. `setLayer` writes one layer's
 * RGBA8 pixels (`size * size * 4` bytes) at mip level 0; `finish`
 * regenerates the mip chain and must follow the last `setLayer` of a batch.
 * `dispose` deletes the texture.
 */
export interface TextureArray {
  texture: WebGLTexture;
  setLayer(index: number, pixels: Uint8Array): void;
  finish(): void;
  dispose(): void;
}

/**
 * Allocates immutable storage for `layers` square RGBA8 layers of `size`
 * pixels with the full mip chain, trilinear filtering, repeating edges and
 * 4x anisotropy where the extension exists. Layers start undefined until
 * `setLayer` fills them. Leaves no texture array bound.
 */
export function createTextureArray(
  gl: WebGL2RenderingContext,
  size: number,
  layers: number,
): TextureArray {
  const texture = gl.createTexture();
  const levels = Math.floor(Math.log2(size)) + 1;
  gl.bindTexture(gl.TEXTURE_2D_ARRAY, texture);
  gl.texStorage3D(gl.TEXTURE_2D_ARRAY, levels, gl.RGBA8, size, size, layers);
  gl.texParameteri(
    gl.TEXTURE_2D_ARRAY,
    gl.TEXTURE_MIN_FILTER,
    gl.LINEAR_MIPMAP_LINEAR,
  );
  gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_S, gl.REPEAT);
  gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_T, gl.REPEAT);
  const aniso = gl.getExtension("EXT_texture_filter_anisotropic");
  if (aniso !== null) {
    gl.texParameterf(gl.TEXTURE_2D_ARRAY, aniso.TEXTURE_MAX_ANISOTROPY_EXT, 4);
  }
  gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
  return {
    texture,
    setLayer(index, pixels) {
      gl.bindTexture(gl.TEXTURE_2D_ARRAY, texture);
      gl.texSubImage3D(
        gl.TEXTURE_2D_ARRAY,
        0,
        0,
        0,
        index,
        size,
        size,
        1,
        gl.RGBA,
        gl.UNSIGNED_BYTE,
        pixels,
      );
      gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
    },
    finish() {
      gl.bindTexture(gl.TEXTURE_2D_ARRAY, texture);
      gl.generateMipmap(gl.TEXTURE_2D_ARRAY);
      gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
    },
    dispose: () => gl.deleteTexture(texture),
  };
}
