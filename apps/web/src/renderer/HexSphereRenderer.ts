/**
 * WebGL2 球面六角格渲染器。
 *
 * 目标：把 65 万到 590 万个六角格实时画成一颗星球，稳定 60fps。
 *
 * 核心技术是**实例化渲染**：一次 draw call 画完全部格子。
 * 顶点着色器里根据实例的球面方向和角点参数算出六边形的位置，
 * 所以 CPU 只需要传「每格的中心方向 + 高程 + 生物群系」三个数组，
 * 不需要传任何顶点缓冲。
 */

import { FRAG, VERT } from './shaders'
import { lookAt, mat4Mul, perspective, type Vec3 } from './math'

export interface CellData {
  /** 格子中心方向 xyz 交错，长度 = count * 3 */
  centers: Float32Array
  /** 每格高程，米 */
  heights: Float32Array
  /** 每格生物群系编号 */
  biomes: Uint8Array
  /** 每格温度（摄氏度）。可选。 */
  temperatures?: Float32Array
  /** 每格降水（毫米/年）。可选。 */
  precipitations?: Float32Array
  count: number
}

/** 地图风格。 */
export enum MapStyle {
  Realistic = 0,
  Watercolor = 1,
  Parchment = 2,
  Contour = 3,
}

export class HexSphereRenderer {
  private gl: WebGL2RenderingContext
  private program: WebGLProgram | null = null
  private vao: WebGLVertexArrayObject | null = null
  private cornerBuf: WebGLBuffer | null = null
  private instCenterBuf: WebGLBuffer | null = null
  private instHeightBuf: WebGLBuffer | null = null
  private instBiomeBuf: WebGLBuffer | null = null
  private instTempBuf: WebGLBuffer | null = null
  private instPrecipBuf: WebGLBuffer | null = null

  private cellCount = 0
  private hexRadius = 0.02

  // 相机状态
  private yaw = 0
  private pitch = 0.35
  private distance = 3.2
  private targetDistance = 3.2
  private autoRotate = true

  // 渲染参数
  public style = MapStyle.Realistic
  /** 当前图层：0=地形 1=温度 2=降水 3=生物群系 4=高程 */
  public layer = 0
  public radius = 1.0
  public heightScale = 0.00006
  public seaLevel = 0.0

  /** 实时预览帧纹理。生成过程中用它覆盖真实数据。 */
  private frameTexture: WebGLTexture | null = null
  private frameW = 0
  private frameH = 0
  private previewMode = false
  private previewMinElev = 0
  private previewMaxElev = 1

  /** 平面补丁模式：子图用平面网格渲染而不是整球。 */
  private planarMode = false
  private planarSize = 0
  /** 平面模式的正交相机半高，由网格尺寸决定。 */
  public planarHalfHeight = 1.0

  private running = false
  private lastTime = 0
  private frames = 0
  private fps = 0
  private rafId = 0

  constructor(private canvas: HTMLCanvasElement) {
    const gl = canvas.getContext('webgl2', {
      antialias: true,
      alpha: false,
      powerPreference: 'high-performance',
    })
    if (!gl) throw new Error('WebGL2 不可用，请更换浏览器')
    this.gl = gl
    this.initGL()
  }

  private initGL() {
    const gl = this.gl

    const vs = this.compileShader(gl.VERTEX_SHADER, VERT)
    const fs = this.compileShader(gl.FRAGMENT_SHADER, FRAG)
    const prog = gl.createProgram()!
    gl.attachShader(prog, vs)
    gl.attachShader(prog, fs)
    gl.linkProgram(prog)
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
      throw new Error('着色器链接失败: ' + gl.getProgramInfoLog(prog))
    }
    this.program = prog

    // 六边形角点：7 个点构成三角扇
    const corners: number[] = []
    for (let i = 0; i <= 6; i++) {
      const a = (i / 6) * Math.PI * 2 + Math.PI / 6
      corners.push(Math.cos(a), Math.sin(a))
    }
    this.cornerBuf = gl.createBuffer()
    gl.bindBuffer(gl.ARRAY_BUFFER, this.cornerBuf)
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(corners), gl.STATIC_DRAW)

    this.vao = gl.createVertexArray()
    gl.bindVertexArray(this.vao)
    gl.bindBuffer(gl.ARRAY_BUFFER, this.cornerBuf)
    gl.enableVertexAttribArray(3)
    gl.vertexAttribPointer(3, 2, gl.FLOAT, false, 0, 0)
    gl.bindVertexArray(null)

    this.frameTexture = gl.createTexture()
    gl.bindTexture(gl.TEXTURE_2D, this.frameTexture)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)

    gl.enable(gl.DEPTH_TEST)
    gl.enable(gl.CULL_FACE)
    gl.cullFace(gl.BACK)
    gl.clearColor(0.039, 0.047, 0.063, 1)
  }

  private compileShader(type: number, src: string): WebGLShader {
    const gl = this.gl
    const s = gl.createShader(type)!
    gl.shaderSource(s, src)
    gl.compileShader(s)
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      throw new Error('着色器编译失败: ' + gl.getShaderInfoLog(s))
    }
    return s
  }

  /**
   * 上传一帧实时预览数据。
   *
   * 帧是等距圆柱投影图像，每像素 5 字节：
   *   高程（u16 LE）、温度（u8 偏移128）、降水（u8 对数）、流量（u8 对数）
   *
   * 生成过程中用这个纹理做屏幕上采样，让用户看到星球在演化。
   */
  uploadFrame(
    data: Uint8Array,
    w: number,
    h: number,
    stats: { land_fraction: number; max_elevation: number; min_elevation: number }
  ) {
    const gl = this.gl
    if (!this.frameTexture) return

    const rgba = new Uint8Array(w * h * 4)
    const eMin = stats.min_elevation
    const eSpan = Math.max(stats.max_elevation - eMin, 1)

    for (let i = 0; i < w * h; i++) {
      const src = i * 5
      const e16 = data[src] | (data[src + 1] << 8)
      rgba[i * 4] = Math.round((e16 / 65535) * 255)
      rgba[i * 4 + 1] = data[src + 2]
      rgba[i * 4 + 2] = data[src + 3]
      rgba[i * 4 + 3] = data[src + 4]
    }

    gl.bindTexture(gl.TEXTURE_2D, this.frameTexture)
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, rgba)

    this.frameW = w
    this.frameH = h
    this.previewMode = true
    this.previewMinElev = eMin
    this.previewMaxElev = stats.max_elevation
  }

  /**
   * 切换到平面补丁模式（用于大陆/区域/地点子图）。
   *
   * 子图不是整球，而是球面上的一块区域，展开成平面网格渲染。
   * 相机改为俯视，用户看到的是「一张地图」而不是「一颗星球」。
   */
  setPlanarMode(size: number) {
    this.planarMode = true
    this.planarSize = size
    this.targetDistance = 2.0
    this.distance = 2.0
    this.pitch = Math.PI / 2 - 0.02
    this.autoRotate = false
  }

  /** 切回球面模式。 */
  setSphereMode() {
    this.planarMode = false
    this.targetDistance = 3.2
    this.pitch = 0.35
    this.autoRotate = true
  }

  isPlanarMode(): boolean {
    return this.planarMode
  }

  /** 退出预览模式，用真实格子数据渲染。 */
  exitPreview() {
    this.previewMode = false
  }

  isPreviewMode(): boolean {
    return this.previewMode
  }

  /** 返回画布元素，供截图等操作使用。 */
  getCanvas(): HTMLCanvasElement {
    return this.canvas
  }

  /** 上传格子数据。 */
  setCells(data: CellData, hexRadiusOverride?: number) {
    const gl = this.gl
    this.cellCount = data.count

    if (hexRadiusOverride !== undefined) {
      // 平面模式：格子半径由平面网格的格间距直接给出
      this.hexRadius = hexRadiusOverride
    } else {
      // 球面模式：单位球面积除以格数得到格面积，
      // 六边形面积 = (3*sqrt(3)/2) * r^2，反解 r。
      const cellArea = (4 * Math.PI) / Math.max(data.count, 1)
      this.hexRadius = Math.sqrt(cellArea / 2.598) * 1.04
    }

    if (!this.instCenterBuf) this.instCenterBuf = gl.createBuffer()
    gl.bindBuffer(gl.ARRAY_BUFFER, this.instCenterBuf)
    gl.bufferData(gl.ARRAY_BUFFER, data.centers, gl.STATIC_DRAW)

    if (!this.instHeightBuf) this.instHeightBuf = gl.createBuffer()
    gl.bindBuffer(gl.ARRAY_BUFFER, this.instHeightBuf)
    gl.bufferData(gl.ARRAY_BUFFER, data.heights, gl.STATIC_DRAW)

    const biomeF = new Float32Array(data.biomes.length)
    for (let i = 0; i < data.biomes.length; i++) biomeF[i] = data.biomes[i]
    if (!this.instBiomeBuf) this.instBiomeBuf = gl.createBuffer()
    gl.bindBuffer(gl.ARRAY_BUFFER, this.instBiomeBuf)
    gl.bufferData(gl.ARRAY_BUFFER, biomeF, gl.STATIC_DRAW)

    gl.bindVertexArray(this.vao)

    gl.bindBuffer(gl.ARRAY_BUFFER, this.instCenterBuf)
    gl.enableVertexAttribArray(0)
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 12, 0)
    gl.vertexAttribDivisor(0, 1)

    gl.bindBuffer(gl.ARRAY_BUFFER, this.instHeightBuf)
    gl.enableVertexAttribArray(1)
    gl.vertexAttribPointer(1, 1, gl.FLOAT, false, 4, 0)
    gl.vertexAttribDivisor(1, 1)

    gl.bindBuffer(gl.ARRAY_BUFFER, this.instBiomeBuf)
    gl.enableVertexAttribArray(2)
    gl.vertexAttribPointer(2, 1, gl.FLOAT, false, 4, 0)
    gl.vertexAttribDivisor(2, 1)

    // 温度（attribute 4）
    if (data.temperatures) {
      if (!this.instTempBuf) this.instTempBuf = gl.createBuffer()
      gl.bindBuffer(gl.ARRAY_BUFFER, this.instTempBuf)
      gl.bufferData(gl.ARRAY_BUFFER, data.temperatures, gl.STATIC_DRAW)
      gl.enableVertexAttribArray(4)
      gl.vertexAttribPointer(4, 1, gl.FLOAT, false, 4, 0)
      gl.vertexAttribDivisor(4, 1)
    }

    // 降水（attribute 5）
    if (data.precipitations) {
      if (!this.instPrecipBuf) this.instPrecipBuf = gl.createBuffer()
      gl.bindBuffer(gl.ARRAY_BUFFER, this.instPrecipBuf)
      gl.bufferData(gl.ARRAY_BUFFER, data.precipitations, gl.STATIC_DRAW)
      gl.enableVertexAttribArray(5)
      gl.vertexAttribPointer(5, 1, gl.FLOAT, false, 4, 0)
      gl.vertexAttribDivisor(5, 1)
    }

    gl.bindVertexArray(null)
  }

  start() {
    if (this.running) return
    this.running = true
    this.lastTime = performance.now()
    const loop = () => {
      if (!this.running) return
      this.render()
      this.rafId = requestAnimationFrame(loop)
    }
    this.rafId = requestAnimationFrame(loop)
  }

  stop() {
    this.running = false
    if (this.rafId) cancelAnimationFrame(this.rafId)
  }

  dispose() {
    this.stop()
    const gl = this.gl
    if (this.program) gl.deleteProgram(this.program)
    if (this.vao) gl.deleteVertexArray(this.vao)
    for (const b of [this.cornerBuf, this.instCenterBuf, this.instHeightBuf, this.instBiomeBuf]) {
      if (b) gl.deleteBuffer(b)
    }
  }

  /** 拖拽旋转。 */
  rotate(dx: number, dy: number) {
    this.autoRotate = false
    this.yaw -= dx * 0.005
    this.pitch = Math.max(-1.5, Math.min(1.5, this.pitch + dy * 0.005))
  }

  /** 缩放。delta 大于 0 表示放大。 */
  zoom(delta: number) {
    this.targetDistance = Math.max(1.4, Math.min(8.0, this.targetDistance * (1 - delta * 0.0012)))
  }

  setAutoRotate(on: boolean) {
    this.autoRotate = on
  }

  getFps(): number {
    return this.fps
  }

  /** 当前缩放级别，用于判断是否需要加载更细的地图。 */
  getZoomLevel(): number {
    return 3.2 / this.distance
  }

  private render() {
    const gl = this.gl
    const now = performance.now()
    const dt = now - this.lastTime
    this.lastTime = now
    this.frames++
    if (this.frames % 20 === 0 && dt > 0) {
      this.fps = Math.round(1000 / dt)
    }

    if (this.autoRotate && !this.planarMode) this.yaw += 0.0008
    this.distance += (this.targetDistance - this.distance) * 0.12

    const dpr = Math.min(window.devicePixelRatio || 1, 2)
    const w = Math.floor(this.canvas.clientWidth * dpr)
    const h = Math.floor(this.canvas.clientHeight * dpr)
    if (w > 0 && h > 0 && (this.canvas.width !== w || this.canvas.height !== h)) {
      this.canvas.width = w
      this.canvas.height = h
    }
    gl.viewport(0, 0, this.canvas.width, this.canvas.height)
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT)

    if (!this.program || this.cellCount === 0) return

    let eye: Vec3
    if (this.planarMode) {
      // 平面模式：正对平面（+z 方向）俯视，相机距离随缩放变化
      eye = [0, 0, this.distance]
    } else {
      const cp = Math.cos(this.pitch)
      eye = [
        Math.sin(this.yaw) * cp * this.distance,
        Math.sin(this.pitch) * this.distance,
        Math.cos(this.yaw) * cp * this.distance,
      ]
    }
    const view = lookAt(eye, [0, 0, 0], [0, 1, 0])
    const aspect = this.canvas.width / Math.max(this.canvas.height, 1)
    // 平面模式用正交投影：格子大小严格一致，不会因透视产生远近差异
    const proj = this.planarMode
      ? orthographic(this.planarHalfHeight, aspect, -10, 10)
      : perspective(Math.PI / 4, aspect, 0.01, 100)
    const viewProj = mat4Mul(proj, view)

    gl.useProgram(this.program)
    gl.bindVertexArray(this.vao)

    const u = (name: string) => gl.getUniformLocation(this.program!, name)
    gl.uniformMatrix4fv(u('uViewProj'), false, viewProj)
    gl.uniform1f(u('uRadius'), this.radius)
    gl.uniform1f(u('uHeightScale'), this.heightScale)
    gl.uniform1f(u('uHexRadius'), this.hexRadius)
    gl.uniform1i(u('uPlanarMode'), this.planarMode ? 1 : 0)
    if (this.planarMode) {
      gl.uniform3f(u('uLightDir'), -0.45, 0.55, 0.70)
    } else {
      gl.uniform3f(u('uLightDir'), -0.4, 0.6, 0.7)
    }
    gl.uniform1f(u('uSeaLevel'), this.seaLevel)
    gl.uniform1i(u('uStyle'), this.style)
    gl.uniform1i(u('uLayer'), this.layer)

    // 预览模式：绑定帧纹理
    gl.uniform1i(u('uPreviewMode'), this.previewMode ? 1 : 0)
    if (this.previewMode && this.frameTexture) {
      gl.activeTexture(gl.TEXTURE0)
      gl.bindTexture(gl.TEXTURE_2D, this.frameTexture)
      gl.uniform1i(u('uFrameTex'), 0)
      gl.uniform2f(u('uFrameSize'), this.frameW, this.frameH)
      gl.uniform1f(u('uPreviewMinElev'), this.previewMinElev)
      gl.uniform1f(u('uPreviewMaxElev'), this.previewMaxElev)
    }

    gl.drawArraysInstanced(gl.TRIANGLE_FAN, 0, 7, this.cellCount)
    gl.bindVertexArray(null)
  }
}


/** 正交投影。halfHeight 是半高（世界单位）。 */
function orthographic(halfHeight: number, aspect: number, near: number, far: number): Float32Array {
  const halfWidth = halfHeight * aspect
  const l = -halfWidth
  const r = halfWidth
  const b = -halfHeight
  const tp = halfHeight
  const nf = 1 / (near - far)
  return new Float32Array([
    2 / (r - l), 0, 0, 0,
    0, 2 / (tp - b), 0, 0,
    0, 0, 2 * nf, 0,
    -(r + l) / (r - l), -(tp + b) / (tp - b), (far + near) * nf, 1,
  ])
}
