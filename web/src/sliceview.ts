import type { Window } from "./windowing";

const VERT_SRC = `#version 300 es
in vec2 aPos;
in vec2 aUv;
out vec2 vUv;
void main() {
  vUv = aUv;
  gl_Position = vec4(aPos, 0.0, 1.0);
}
`;

const FRAG_SRC = `#version 300 es
precision highp float;
precision highp int;
precision highp isampler2D;
uniform isampler2D uSlice;
uniform float uCenter;
uniform float uWidth;
in vec2 vUv;
out vec4 frag;
void main() {
  float hu = float(texture(uSlice, vUv).r);
  float safeWidth = uWidth <= 0.0 ? 1.0 : uWidth;
  float g = clamp((hu - (uCenter - safeWidth * 0.5)) / safeWidth, 0.0, 1.0);
  frag = vec4(g, g, g, 1.0);
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

export class SliceView {
  private gl: WebGL2RenderingContext;
  private program: WebGLProgram;
  private texture: WebGLTexture;
  private uCenterLoc: WebGLUniformLocation | null;
  private uWidthLoc: WebGLUniformLocation | null;
  private texRows = 0;
  private texCols = 0;

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

    // Full-screen quad. UVs are chosen so that row 0 of the uploaded pixel
    // buffer (the DICOM buffer's first row) lands at the top of the canvas.
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

    const texture = gl.createTexture();
    if (!texture) throw new Error("failed to create texture");
    this.texture = texture;
    gl.bindTexture(gl.TEXTURE_2D, texture);
    // Integer textures are not filterable in WebGL2: LINEAR produces an
    // incomplete texture that samples as black. NEAREST is mandatory.
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

    this.uCenterLoc = gl.getUniformLocation(program, "uCenter");
    this.uWidthLoc = gl.getUniformLocation(program, "uWidth");
    const uSliceLoc = gl.getUniformLocation(program, "uSlice");
    gl.useProgram(program);
    gl.uniform1i(uSliceLoc, 0);
  }

  uploadSlice(pixels: Int16Array, rows: number, cols: number): void {
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    if (rows === this.texRows && cols === this.texCols) {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, cols, rows, gl.RED_INTEGER, gl.SHORT, pixels);
    } else {
      gl.texImage2D(
        gl.TEXTURE_2D,
        0,
        gl.R16I,
        cols,
        rows,
        0,
        gl.RED_INTEGER,
        gl.SHORT,
        pixels
      );
      this.texRows = rows;
      this.texCols = cols;
    }
  }

  setWindow(w: Window): void {
    const gl = this.gl;
    gl.useProgram(this.program);
    gl.uniform1f(this.uCenterLoc, w.center);
    gl.uniform1f(this.uWidthLoc, w.width);
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

    if (this.texCols === 0 || this.texRows === 0) return;

    // Letterbox: keep the image's true aspect ratio inside the canvas
    // rather than stretching to fill it.
    const canvasAspect = canvas.width / canvas.height;
    const imageAspect = this.texCols / this.texRows;
    let vx = 0;
    let vy = 0;
    let vw = canvas.width;
    let vh = canvas.height;
    if (canvasAspect > imageAspect) {
      vw = Math.round(canvas.height * imageAspect);
      vx = Math.round((canvas.width - vw) / 2);
    } else {
      vh = Math.round(canvas.width / imageAspect);
      vy = Math.round((canvas.height - vh) / 2);
    }
    gl.viewport(vx, vy, vw, vh);

    gl.useProgram(this.program);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
  }
}
