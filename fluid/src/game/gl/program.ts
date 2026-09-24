/**
 * Shader programs, compiled and linked with their logs kept.
 *
 * A GLSL error is a programming error, so it throws with the driver's
 * message and the numbered source, which is the fastest way to find the
 * line in a string literal. Uniform locations are looked up once and cached
 * by name.
 */

/**
 * A linked program. `uniform` returns the location of a uniform by name,
 * cached after the first lookup; null means the name is unknown or the
 * compiler optimised the uniform away, which WebGL's `uniform*` calls accept
 * as a no-op. `dispose` deletes the program.
 */
export interface Program {
  program: WebGLProgram;
  uniform(name: string): WebGLUniformLocation | null;
  dispose(): void;
}

function numbered(source: string): string {
  return source
    .split("\n")
    .map((line, i) => `${String(i + 1).padStart(3)} ${line}`)
    .join("\n");
}

function compile(
  gl: WebGL2RenderingContext,
  type: GLenum,
  source: string,
): WebGLShader {
  const shader = gl.createShader(type);
  if (shader === null) throw new Error("createShader failed");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (gl.getShaderParameter(shader, gl.COMPILE_STATUS) !== true) {
    const log = gl.getShaderInfoLog(shader) ?? "";
    gl.deleteShader(shader);
    throw new Error(`shader compile failed: ${log}\n${numbered(source)}`);
  }
  return shader;
}

/**
 * Compiles both shaders and links them into a program. The shader objects
 * are deleted once linked (the program keeps what it needs). Throws with
 * the compile log and the numbered source, or with the link log, when the
 * GLSL is wrong.
 */
export function createProgram(
  gl: WebGL2RenderingContext,
  vertexSource: string,
  fragmentSource: string,
): Program {
  const vs = compile(gl, gl.VERTEX_SHADER, vertexSource);
  const fs = compile(gl, gl.FRAGMENT_SHADER, fragmentSource);
  const program = gl.createProgram();
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  if (gl.getProgramParameter(program, gl.LINK_STATUS) !== true) {
    const log = gl.getProgramInfoLog(program) ?? "";
    gl.deleteProgram(program);
    throw new Error(`program link failed: ${log}`);
  }
  const cache = new Map<string, WebGLUniformLocation | null>();
  return {
    program,
    uniform(name) {
      if (!cache.has(name))
        cache.set(name, gl.getUniformLocation(program, name));
      return cache.get(name) ?? null;
    },
    dispose: () => gl.deleteProgram(program),
  };
}
