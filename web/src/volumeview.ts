import type { Window } from "./windowing";
import { normalizeHU, physicalExtent, HU_MIN, HU_MAX, MAX_RAYMARCH_STEPS, type HuRange } from "./volumemath";
import { floatToHalf } from "./float16";

const VERT_SRC = `#version 300 es
in vec2 aPos;
in vec2 aUv;
out vec2 vUv;
void main() {
  vUv = aUv;
  gl_Position = vec4(aPos, 0.0, 1.0);
}
`;

// Raymarching fragment shader. Everything is in "box space": the volume's
// physical bounding box, centred at the origin, sized by physicalExtent()
// so the largest axis spans 1.0 and anisotropic voxel spacing is respected.
const FRAG_SRC = `#version 300 es
precision highp float;
precision highp int;
precision highp sampler3D;

uniform sampler3D uVolume;
uniform sampler2D uTransferFunction;
uniform vec3 uBoxMin;
uniform vec3 uBoxMax;
uniform vec3 uInvDim;
uniform vec3 uCameraPos;
uniform vec3 uCameraRight;
uniform vec3 uCameraUp;
uniform vec3 uCameraForward;
uniform float uTanHalfFov;
uniform float uAspect;
uniform int uSteps;
uniform float uOpacityScale;
uniform float uThresholdHU;
uniform float uReferenceStep;
uniform int uMipMode;
uniform float uWindowCenter;
uniform float uWindowWidth;
// The volume's actual HU range (server-reported hu_min/hu_max, not a fixed
// clinical constant — see volumemath.ts's normalizeHU). Must match the range
// uploadVolume() normalised the 3D texture with, or the HU reconstruction
// below disagrees with what's actually in the texture.
uniform float uHuMin;
uniform float uHuRange;

in vec2 vUv;
out vec4 frag;

// Hard cap on the loop trip count. WebGL2/GLSL ES 3.00 fragment shaders do
// allow non-constant loop bounds, but a fixed upper bound with an early
// break is the safest pattern across driver compilers. uSteps (the quality
// slider) is clamped to this same ceiling on the JS side (see
// volumemath.ts's MAX_RAYMARCH_STEPS) so a deep volume's computed default
// can approach but never exceed it.
const int MAX_STEPS = 2048;

vec3 gradientAt(vec3 tc) {
  float dx = texture(uVolume, tc + vec3(uInvDim.x, 0.0, 0.0)).r
           - texture(uVolume, tc - vec3(uInvDim.x, 0.0, 0.0)).r;
  float dy = texture(uVolume, tc + vec3(0.0, uInvDim.y, 0.0)).r
           - texture(uVolume, tc - vec3(0.0, uInvDim.y, 0.0)).r;
  float dz = texture(uVolume, tc + vec3(0.0, 0.0, uInvDim.z)).r
           - texture(uVolume, tc - vec3(0.0, 0.0, uInvDim.z)).r;
  // Density gradient points toward increasing HU; the outward surface
  // normal (toward decreasing density, e.g. bone -> soft tissue) is its
  // negation.
  return -vec3(dx, dy, dz);
}

bool intersectBox(vec3 ro, vec3 rd, out float tNear, out float tFar) {
  vec3 invD = 1.0 / rd;
  vec3 t0 = (uBoxMin - ro) * invD;
  vec3 t1 = (uBoxMax - ro) * invD;
  vec3 tmin = min(t0, t1);
  vec3 tmax = max(t0, t1);
  tNear = max(max(tmin.x, tmin.y), tmin.z);
  tFar = min(min(tmax.x, tmax.y), tmax.z);
  return tFar > max(tNear, 0.0);
}

void main() {
  vec2 ndc = vUv * 2.0 - 1.0;
  vec3 rayDir = normalize(
    uCameraForward
    + ndc.x * uTanHalfFov * uAspect * uCameraRight
    + ndc.y * uTanHalfFov * uCameraUp
  );
  vec3 rayOrigin = uCameraPos;

  float tNear, tFar;
  if (!intersectBox(rayOrigin, rayDir, tNear, tFar)) {
    frag = vec4(0.0, 0.0, 0.0, 1.0);
    return;
  }
  tNear = max(tNear, 0.0);

  float stepSize = (tFar - tNear) / float(uSteps);
  vec3 accumColor = vec3(0.0);
  float accumAlpha = 0.0;
  float mipValue = 0.0;
  float t = tNear + stepSize * 0.5;

  for (int i = 0; i < MAX_STEPS; i++) {
    if (i >= uSteps || t > tFar) break;

    vec3 pos = rayOrigin + rayDir * t;
    vec3 tc = (pos - uBoxMin) / (uBoxMax - uBoxMin);
    float n = texture(uVolume, tc).r;

    if (uMipMode == 1) {
      mipValue = max(mipValue, n);
    } else {
      float hu = n * uHuRange + uHuMin;
      if (hu >= uThresholdHU) {
        vec4 tf = texture(uTransferFunction, vec2(n, 0.5));
        float aSample = tf.a * uOpacityScale;
        if (aSample > 0.001) {
          vec3 normal = normalize(gradientAt(tc));
          vec3 viewDir = normalize(uCameraPos - pos);
          // Headlamp lighting: light travels with the camera, so the surface
          // always reads even from angles a fixed light would leave dark.
          vec3 lightDir = viewDir;
          float diff = max(dot(normal, lightDir), 0.0);
          vec3 halfV = normalize(lightDir + viewDir);
          float spec = pow(max(dot(normal, halfV), 0.0), 32.0);
          vec3 lit = tf.rgb * (0.35 + 0.65 * diff) + vec3(spec) * 0.3;

          // Opacity correction for step size: a_i = 1 - (1-a)^(step/ref), so
          // the quality slider changes noise/fidelity, not overall density.
          float aCorrected = 1.0 - pow(1.0 - aSample, stepSize / uReferenceStep);

          accumColor += (1.0 - accumAlpha) * lit * aCorrected;
          accumAlpha += (1.0 - accumAlpha) * aCorrected;

          // Early ray termination: deliberate, and the main perf win. Once
          // the ray is ~opaque, samples behind it are invisible under
          // front-to-back compositing, so stop marching.
          if (accumAlpha > 0.99) break;
        }
      }
    }

    t += stepSize;
  }

  if (uMipMode == 1) {
    float hu = mipValue * uHuRange + uHuMin;
    float safeWidth = uWindowWidth <= 0.0 ? 1.0 : uWindowWidth;
    float g = clamp((hu - (uWindowCenter - safeWidth * 0.5)) / safeWidth, 0.0, 1.0);
    frag = vec4(g, g, g, 1.0);
  } else {
    frag = vec4(accumColor, 1.0);
  }
}
`;

function compileShader(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("failed to create shader");
  gl.shaderSource(shader, src);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(shader);
    gl.deleteShader(shader);
    throw new Error(`shader compile error: ${log}`);
  }
  return shader;
}

interface Vec3 {
  x: number;
  y: number;
  z: number;
}

function sub(a: Vec3, b: Vec3): Vec3 {
  return { x: a.x - b.x, y: a.y - b.y, z: a.z - b.z };
}
function cross(a: Vec3, b: Vec3): Vec3 {
  return { x: a.y * b.z - a.z * b.y, y: a.z * b.x - a.x * b.z, z: a.x * b.y - a.y * b.x };
}
function normalize(a: Vec3): Vec3 {
  const len = Math.hypot(a.x, a.y, a.z) || 1;
  return { x: a.x / len, y: a.y / len, z: a.z / len };
}

const MIN_ELEVATION = -Math.PI / 2 + 0.05;
const MAX_ELEVATION = Math.PI / 2 - 0.05;

export class VolumeView {
  private gl: WebGL2RenderingContext;
  private program: WebGLProgram;
  private volumeTexture: WebGLTexture;
  private lutTexture: WebGLTexture;
  private uniforms: Record<string, WebGLUniformLocation | null> = {};

  private dimX = 0;
  private dimY = 0;
  private dimZ = 0;
  private boxMin: Vec3 = { x: -0.5, y: -0.5, z: -0.5 };
  private boxMax: Vec3 = { x: 0.5, y: 0.5, z: 0.5 };
  private referenceStep = 1 / 256;
  // HU range the currently-uploaded volume texture was normalised with;
  // defaults to the fixed clinical range until a real volume is loaded.
  private huMin: number = HU_MIN;
  private huRangeSpan: number = HU_MAX - HU_MIN;

  // Orbit camera state, spherical around the box centre (origin).
  private azimuth = 0.6;
  private elevation = 0.35;
  private distance = 2.2;

  private steps = 256;
  private opacityScale = 1.0;
  private thresholdHU = -1024;
  private mipMode = false;
  private window: Window = { center: 300, width: 1500 };

  constructor(private canvas: HTMLCanvasElement) {
    const gl = canvas.getContext("webgl2", { antialias: false });
    if (!gl) throw new Error("WebGL2 is not supported in this browser");
    this.gl = gl;

    const vs = compileShader(gl, gl.VERTEX_SHADER, VERT_SRC);
    const fs = compileShader(gl, gl.FRAGMENT_SHADER, FRAG_SRC);
    const program = gl.createProgram();
    if (!program) throw new Error("failed to create program");
    gl.attachShader(program, vs);
    gl.attachShader(program, fs);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      throw new Error(`program link error: ${gl.getProgramInfoLog(program)}`);
    }
    this.program = program;

    // prettier-ignore
    const verts = new Float32Array([
      -1, -1, 0, 1,
       1, -1, 1, 1,
      -1,  1, 0, 0,
      -1,  1, 0, 0,
       1, -1, 1, 1,
       1,  1, 1, 0,
    ]);
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    const vbo = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
    gl.bufferData(gl.ARRAY_BUFFER, verts, gl.STATIC_DRAW);
    const aPos = gl.getAttribLocation(program, "aPos");
    const aUv = gl.getAttribLocation(program, "aUv");
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 16, 0);
    gl.enableVertexAttribArray(aUv);
    gl.vertexAttribPointer(aUv, 2, gl.FLOAT, false, 16, 8);
    gl.bindVertexArray(vao);

    const volumeTexture = gl.createTexture();
    if (!volumeTexture) throw new Error("failed to create volume texture");
    this.volumeTexture = volumeTexture;
    gl.bindTexture(gl.TEXTURE_3D, volumeTexture);
    // R16F + HALF_FLOAT (not R16I like the 2D slice path) because integer
    // textures aren't filterable in WebGL2, and raymarching without
    // trilinear filtering produces heavy blocky aliasing. HU is normalised
    // to [0,1] on the CPU first (over the volume's actual hu_min/hu_max, see
    // uploadVolume); half-float's ~11-bit mantissa over [0,1] is lossless
    // against the low-thousands of distinct HU values any real CT range has.
    gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_WRAP_R, gl.CLAMP_TO_EDGE);

    const lutTexture = gl.createTexture();
    if (!lutTexture) throw new Error("failed to create LUT texture");
    this.lutTexture = lutTexture;
    gl.bindTexture(gl.TEXTURE_2D, lutTexture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

    for (const name of [
      "uVolume",
      "uTransferFunction",
      "uBoxMin",
      "uBoxMax",
      "uInvDim",
      "uCameraPos",
      "uCameraRight",
      "uCameraUp",
      "uCameraForward",
      "uTanHalfFov",
      "uAspect",
      "uSteps",
      "uOpacityScale",
      "uThresholdHU",
      "uReferenceStep",
      "uMipMode",
      "uWindowCenter",
      "uWindowWidth",
      "uHuMin",
      "uHuRange",
    ]) {
      this.uniforms[name] = gl.getUniformLocation(program, name);
    }

    gl.useProgram(program);
    gl.uniform1i(this.uniforms.uVolume, 0);
    gl.uniform1i(this.uniforms.uTransferFunction, 1);
  }

  /**
   * Uploads a raw HU volume (x fastest, then y, then z) and sizes the box.
   * `range` is the volume's actual hu_min/hu_max (server-reported); it's
   * what the CPU-side normalisation below uses, and it must be handed to
   * the shader (see render()) so its HU reconstruction agrees with what got
   * baked into the texture. Defaults to the fixed clinical range for
   * callers that don't have a per-volume range.
   */
  uploadVolume(
    voxels: Int16Array,
    dimX: number,
    dimY: number,
    dimZ: number,
    spacingX: number,
    spacingY: number,
    spacingZ: number,
    range: HuRange = { min: HU_MIN, max: HU_MAX }
  ): void {
    const gl = this.gl;
    this.dimX = dimX;
    this.dimY = dimY;
    this.dimZ = dimZ;
    this.huMin = range.min;
    this.huRangeSpan = range.max - range.min;

    const half = new Uint16Array(voxels.length);
    for (let i = 0; i < voxels.length; i++) {
      half[i] = floatToHalf(normalizeHU(voxels[i], range));
    }

    gl.bindTexture(gl.TEXTURE_3D, this.volumeTexture);
    // Half-float texels are 2 bytes; default UNPACK_ALIGNMENT of 4 can
    // misalign odd-width rows.
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 2);
    gl.texImage3D(
      gl.TEXTURE_3D,
      0,
      gl.R16F,
      dimX,
      dimY,
      dimZ,
      0,
      gl.RED,
      gl.HALF_FLOAT,
      half
    );
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);

    const extent = physicalExtent(dimX, dimY, dimZ, spacingX, spacingY, spacingZ);
    this.boxMin = { x: -extent.x / 2, y: -extent.y / 2, z: -extent.z / 2 };
    this.boxMax = { x: extent.x / 2, y: extent.y / 2, z: extent.z / 2 };
    const diagonal = Math.hypot(extent.x, extent.y, extent.z);
    this.referenceStep = diagonal / 256;
  }

  setTransferFunction(lut: Uint8Array): void {
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, this.lutTexture);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA,
      lut.length / 4,
      1,
      0,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      lut
    );
  }

  setSteps(steps: number): void {
    this.steps = Math.min(MAX_RAYMARCH_STEPS, Math.max(8, Math.round(steps)));
  }

  /** Density multiplier on the transfer function's authored alpha; bounded [0,1] — opacity can't exceed "as authored". */
  setOpacityScale(scale: number): void {
    this.opacityScale = Math.min(1, Math.max(0, scale));
  }

  setThresholdHU(hu: number): void {
    this.thresholdHU = hu;
  }

  setMipMode(enabled: boolean): void {
    this.mipMode = enabled;
  }

  setWindow(w: Window): void {
    this.window = w;
  }

  /** Left-drag orbit: dAz/dEl in radians. Pitch is clamped to avoid pole flip; no roll. */
  orbit(dAzimuth: number, dElevation: number): void {
    this.azimuth += dAzimuth;
    this.elevation = Math.min(MAX_ELEVATION, Math.max(MIN_ELEVATION, this.elevation + dElevation));
  }

  zoom(factor: number): void {
    this.distance = Math.min(10, Math.max(0.3, this.distance * factor));
  }

  private cameraPosition(): Vec3 {
    return {
      x: this.distance * Math.cos(this.elevation) * Math.sin(this.azimuth),
      y: this.distance * Math.sin(this.elevation),
      z: this.distance * Math.cos(this.elevation) * Math.cos(this.azimuth),
    };
  }

  render(): void {
    const gl = this.gl;
    const canvas = this.canvas;
    const dpr = window.devicePixelRatio || 1;
    const displayWidth = Math.round(canvas.clientWidth * dpr);
    const displayHeight = Math.round(canvas.clientHeight * dpr);
    if (canvas.width !== displayWidth || canvas.height !== displayHeight) {
      canvas.width = displayWidth;
      canvas.height = displayHeight;
    }
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.clearColor(0, 0, 0, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);

    if (this.dimX === 0) return;

    const camPos = this.cameraPosition();
    const forward = normalize(sub({ x: 0, y: 0, z: 0 }, camPos));
    const worldUp: Vec3 = { x: 0, y: 1, z: 0 };
    const right = normalize(cross(forward, worldUp));
    const up = cross(right, forward);

    gl.useProgram(this.program);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_3D, this.volumeTexture);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.lutTexture);

    gl.uniform3f(this.uniforms.uBoxMin, this.boxMin.x, this.boxMin.y, this.boxMin.z);
    gl.uniform3f(this.uniforms.uBoxMax, this.boxMax.x, this.boxMax.y, this.boxMax.z);
    gl.uniform3f(this.uniforms.uInvDim, 1 / this.dimX, 1 / this.dimY, 1 / this.dimZ);
    gl.uniform3f(this.uniforms.uCameraPos, camPos.x, camPos.y, camPos.z);
    gl.uniform3f(this.uniforms.uCameraRight, right.x, right.y, right.z);
    gl.uniform3f(this.uniforms.uCameraUp, up.x, up.y, up.z);
    gl.uniform3f(this.uniforms.uCameraForward, forward.x, forward.y, forward.z);
    gl.uniform1f(this.uniforms.uTanHalfFov, Math.tan((45 * Math.PI) / 180 / 2));
    gl.uniform1f(this.uniforms.uAspect, canvas.width / canvas.height);
    gl.uniform1i(this.uniforms.uSteps, this.steps);
    gl.uniform1f(this.uniforms.uOpacityScale, this.opacityScale);
    gl.uniform1f(this.uniforms.uThresholdHU, this.thresholdHU);
    gl.uniform1f(this.uniforms.uReferenceStep, this.referenceStep);
    gl.uniform1i(this.uniforms.uMipMode, this.mipMode ? 1 : 0);
    gl.uniform1f(this.uniforms.uWindowCenter, this.window.center);
    gl.uniform1f(this.uniforms.uWindowWidth, this.window.width);
    gl.uniform1f(this.uniforms.uHuMin, this.huMin);
    gl.uniform1f(this.uniforms.uHuRange, this.huRangeSpan);

    gl.drawArrays(gl.TRIANGLES, 0, 6);
  }
}
