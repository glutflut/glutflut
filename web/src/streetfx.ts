// Shared WebGL2 bits for the fire/water looks on streets: the map's flow layer
// (flow.ts) and the camera view (ar.ts) use the same GLSL and program setup.

/** GLSL: noise and the looks per owner. u runs along the street (pattern
 *  units), s across it (-1..1), t is time × speed. */
export const LOOKS = /* glsl */ `
float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float noise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1, 0)), f.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), f.x), f.y);
}
// Fire: tongues tapering to a hot tip that runs ahead, fading behind.
void fireLook(float u, float s, float t, out vec3 c, out float i) {
  float side = abs(s);
  float core = 1.0 - side;
  float n = noise(vec2(u * 1.1 - t * 1.3, s * 2.0 + t * 0.7));
  float g = fract(u * 0.9 - t * 1.4 + 0.35 * n);
  float width = mix(1.0, 0.3, g * g);
  float tongue = (1.0 - smoothstep(width * 0.55, width, side)) * (0.25 + 0.75 * pow(g, 2.5));
  i = max(tongue, core * 0.25 * (0.6 + 0.4 * n));
  c = mix(vec3(0.95, 0.22, 0.02), vec3(1.0, 0.9, 0.55), i * i);
}
// Water: waves with a steep front and a foam line ahead, fine ripples.
void waterLook(float u, float s, float t, float base, out vec3 c, out float i) {
  float side = abs(s);
  float core = 1.0 - side;
  float g = fract(u * 0.8 - t * 1.1 + 0.15 * sin(s * 2.0));
  float wave = pow(g, 3.0);
  float foam = smoothstep(0.93, 0.99, g);
  float ripple = 0.5 + 0.5 * sin((u * 2.6 - t * 1.9 + s * 1.7) * 6.2832);
  i = core * core * (base + 0.55 * wave + 0.15 * ripple) + core * 0.6 * foam;
  c = mix(vec3(0.05, 0.38, 0.95), vec3(0.8, 0.96, 1.0), clamp(0.6 * wave + foam, 0.0, 1.0));
}
// Front: sparks where fire meets water, slow and drifting, over a steady base.
void frontLook(float u, float s, float time, out vec3 c, out float i) {
  float core = 1.0 - abs(s);
  float n = noise(vec2(u * 2.5 - time * 0.4, time * 2.2));
  float spark = smoothstep(0.55, 0.9, n);
  i = core * (0.3 + 0.7 * spark);
  c = mix(vec3(0.72, 0.3, 0.95), vec3(0.98, 0.8, 1.0), spark);
}
// Chevrons pointing along the flow, travelling with it.
void chevrons(float u, float s, float t, float k, inout vec3 c, inout float i) {
  float side = abs(s);
  float core = 1.0 - side;
  float cf = fract(u / 3.0 - t * 0.35);
  float chev = (1.0 - smoothstep(0.0, 0.06, abs(cf - (0.35 + 0.3 * core)))) * step(side, 0.8);
  i = max(i, k * chev);
  c = mix(c, vec3(1.0), 0.5 * chev);
}
`;

/** Compile and link; throws with the info log. */
export function program(gl: WebGL2RenderingContext, vs: string, fs: string): WebGLProgram {
  const sh = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader");
    return s;
  };
  const p = gl.createProgram()!;
  gl.attachShader(p, sh(gl.VERTEX_SHADER, vs));
  gl.attachShader(p, sh(gl.FRAGMENT_SHADER, fs));
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) ?? "link");
  return p;
}

export function uniforms(gl: WebGL2RenderingContext, p: WebGLProgram, names: string[]) {
  const u: Record<string, WebGLUniformLocation | null> = {};
  for (const n of names) u[n] = gl.getUniformLocation(p, n);
  return u;
}

/** VAO over one interleaved float buffer (and an index buffer); returns the
 *  buffers. Floats per vertex = sum of the sizes. */
export function interleaved(gl: WebGL2RenderingContext, p: WebGLProgram, attrs: [string, number][]) {
  const vao = gl.createVertexArray()!;
  const vbo = gl.createBuffer()!;
  const ibo = gl.createBuffer()!;
  gl.bindVertexArray(vao);
  gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
  gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo);
  const stride = attrs.reduce((n, [, s]) => n + s, 0) * 4;
  let off = 0;
  for (const [name, size] of attrs) {
    const loc = gl.getAttribLocation(p, name);
    if (loc >= 0) {
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, size, gl.FLOAT, false, stride, off * 4);
    }
    off += size;
  }
  gl.bindVertexArray(null);
  return { vao, vbo, ibo };
}
