/** Continuous globe/plane renderer with an independent shared-edge hex overlay. */
import { FRAG, VERT, LINE_VERT, LINE_FRAG } from './shaders'
import { atlasSurface, atlasLines, projectAtlasPositions } from './surface'
import { projectedWorldDirection, projectedWorldPoint } from './geometry'
import { projectionBounds, projectionIndex, type WorldProjection } from './projections'
import { lookAt, mat4Mul, perspective, cross, dot, norm, type Vec3 } from './math'
import { frameTextureChannels } from './frameTexture'
import { createPigmentTexture } from './pigmentTexture'
import pigmentUrl from '../assets/atlas-pigment.png'
import { heightGradients } from './relief'
import { lakeShoreDepths } from './lakeShore'
import { iceAttributes, type IceData } from '../cryosphere/data'
import { ecosystemAttributes } from '../ecosystems/view'
import type { EcosystemData } from '../ecosystems/data'

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
  flows?: Float32Array
  lakeDepths?: Float32Array
  cryosphere?: IceData
  ecosystems?: EcosystemData
  ecosystemMode?: number
  plateIds?: Float32Array
  indices: Uint32Array
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
  private uniforms = new Map<string, WebGLUniformLocation | null>()
  private lineUniforms = new Map<string, WebGLUniformLocation | null>()
  private vao: WebGLVertexArrayObject | null = null
  private indexBuf: WebGLBuffer | null = null
  private indexCount = 0
  private lineProgram: WebGLProgram | null = null
  private lineVao: WebGLVertexArrayObject | null = null
  private lineBuf: WebGLBuffer | null = null
  private lineGeoBuf: WebGLBuffer | null = null
  private lineCount = 0
  private diagnostics: {
    vao: WebGLVertexArrayObject
    buffer: WebGLBuffer
    geoBuffer: WebGLBuffer
    count: number
  }[] = []
  private instGeoBuf: WebGLBuffer | null = null
  private instPlateBuf: WebGLBuffer | null = null
  public mapMetres = 6371000
  private instCenterBuf: WebGLBuffer | null = null
  private instHeightBuf: WebGLBuffer | null = null
  private instBiomeBuf: WebGLBuffer | null = null
  private instTempBuf: WebGLBuffer | null = null
  private instPrecipBuf: WebGLBuffer | null = null
  private instFlowBuf: WebGLBuffer | null = null
  private instLakeBuf: WebGLBuffer | null = null
  private gradientBuf: WebGLBuffer | null = null
  private iceBuf: WebGLBuffer | null = null
  private frozenBuf: WebGLBuffer | null = null
  private hasCryosphere = false
  private ecosystemColorBuf: WebGLBuffer | null = null
  private ecosystemPaintBuf: WebGLBuffer | null = null
  private hasEcosystems = false
  private disposed = false
  private geographyBuf: WebGLBuffer | null = null
  private geographySourceIds: Uint32Array | undefined
  private geographyActive = false
  private partitionBuf: WebGLBuffer | null = null
  private pan: Vec3 = [0, 0, 0]
  private planarZoom = 1
  private matrix: Float32Array = new Float32Array(16)
  private eye: Vec3 = [0, 0, 3.2]

  private cellCount = 0

  // 相机状态
  private yaw = 0
  private pitch = 0.35
  private distance = 3.2
  private targetDistance = 3.2
  private autoRotate = false

  // 渲染参数
  public style = MapStyle.Parchment
  /** 默认自然彩绘总览；科学图层可在原图层面板切换。 */
  public layer = 10
  public radius = 1.0
  public heightScale = 0.00006

  /** 实时预览帧纹理。生成过程中用它覆盖真实数据。 */
  private frameTexture: WebGLTexture | null = null
  private flowTexture: WebGLTexture | null = null
  private pigment: ReturnType<typeof createPigmentTexture> | null = null
  private previewMode = false
  private previewMinElev = 0
  private previewMaxElev = 1

  /** 平面补丁模式：子图用平面网格渲染而不是整球。 */
  private planarMode = false
  private worldFlatMode = false
  private worldProjection: WorldProjection = 'equirectangular'
  public previewHalfHeight = 1
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

    gl.deleteShader(vs)
    gl.deleteShader(fs)
    this.vao = gl.createVertexArray()
    this.indexBuf = gl.createBuffer()
    this.lineVao = gl.createVertexArray()
    this.lineBuf = gl.createBuffer()
    this.lineGeoBuf = gl.createBuffer()
    const lvs = this.compileShader(gl.VERTEX_SHADER, LINE_VERT),
      lfs = this.compileShader(gl.FRAGMENT_SHADER, LINE_FRAG)
    this.lineProgram = gl.createProgram()!
    gl.attachShader(this.lineProgram, lvs)
    gl.attachShader(this.lineProgram, lfs)
    gl.linkProgram(this.lineProgram)
    if (!gl.getProgramParameter(this.lineProgram, gl.LINK_STATUS))
      throw new Error('网格着色器链接失败: ' + gl.getProgramInfoLog(this.lineProgram))
    gl.deleteShader(lvs)
    gl.deleteShader(lfs)

    this.frameTexture = gl.createTexture()
    this.flowTexture = gl.createTexture()
    this.pigment = createPigmentTexture(gl, pigmentUrl)
    for (const texture of [this.frameTexture, this.flowTexture]) {
      gl.bindTexture(gl.TEXTURE_2D, texture)
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
    }

    gl.enable(gl.DEPTH_TEST)
    gl.enable(gl.CULL_FACE)
    gl.cullFace(gl.BACK)
    gl.clearColor(245 / 255, 242 / 255, 233 / 255, 1)
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
   *   高程（u16 LE）、温度（u8 映射 −40～40℃）、降水（u8 对数）、流量（u8 对数）
   *
   * 生成过程中用这个纹理做屏幕上采样，让用户看到星球在演化。
   */
  uploadFrame(
    data: Uint8Array,
    w: number,
    h: number,
    stats: {
      land_fraction: number
      max_elevation: number
      min_elevation: number
    },
  ) {
    const gl = this.gl
    if (!this.frameTexture || !this.flowTexture) return
    const { rgba, flow } = frameTextureChannels(data, w, h)
    const eMin = stats.min_elevation
    gl.activeTexture(gl.TEXTURE0)
    gl.bindTexture(gl.TEXTURE_2D, this.frameTexture)
    gl.texParameteri(
      gl.TEXTURE_2D,
      gl.TEXTURE_WRAP_S,
      this.planarMode && !this.worldFlatMode ? gl.CLAMP_TO_EDGE : gl.REPEAT,
    )
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, rgba)
    gl.activeTexture(gl.TEXTURE1)
    gl.bindTexture(gl.TEXTURE_2D, this.flowTexture)
    gl.texParameteri(
      gl.TEXTURE_2D,
      gl.TEXTURE_WRAP_S,
      this.planarMode && !this.worldFlatMode ? gl.CLAMP_TO_EDGE : gl.REPEAT,
    )
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1)
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, w, h, 0, gl.RED, gl.UNSIGNED_BYTE, flow)
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4)
    gl.activeTexture(gl.TEXTURE0)

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
  setPlanarMode() {
    this.worldFlatMode = false
    this.planarZoom = 1
    this.pan = [0, 0, 0]
    this.planarMode = true
    this.targetDistance = 2.0
    this.distance = 2.0
    this.pitch = Math.PI / 2 - 0.02
    this.autoRotate = false
  }

  /** World atlas retains the same cell IDs and geographic picking as the globe. */
  setWorldFlatMode(projection: WorldProjection = 'equirectangular') {
    this.setPlanarMode()
    this.worldFlatMode = true
    this.worldProjection = projection
    const bounds = projectionBounds(projection)
    this.planarHalfHeight =
      1.08 *
      Math.max(bounds.height, (bounds.width * this.canvas.clientHeight) / this.canvas.clientWidth)
  }

  /** 切回球面模式。 */
  setSphereMode() {
    this.worldFlatMode = false
    this.autoRotate = false
    this.planarMode = false
    this.targetDistance = 3.2
    this.pitch = 0.35
    this.autoRotate = false
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
  /** Snapshot immediately after drawing; WebGL may clear the buffer later. */
  capturePng(): Promise<Blob> {
    return new Promise((resolve, reject) => {
      requestAnimationFrame(() => {
        try {
          this.render()
          this.canvas.toBlob(
            (blob) => (blob ? resolve(blob) : reject(new Error('地图图像未能生成'))),
            'image/png',
          )
        } catch (e) {
          reject(e)
        }
      })
    })
  }

  /** 上传格子数据。 */
  setCells(data: CellData) {
    const gl = this.gl
    this.cellCount = data.count
    let centers = data.centers,
      indices = data.indices,
      sourceIds: Uint32Array | undefined
    let geoPositions = centers
    if (this.worldFlatMode) {
      const atlas = atlasSurface(centers, indices)
      geoPositions = atlas.positions
      centers = projectAtlasPositions(atlas.positions, this.worldProjection)
      indices = atlas.indices
      sourceIds = atlas.sourceIds
    }
    this.indexCount = indices.length
    this.geographySourceIds = sourceIds
    this.geographyActive = false
    gl.bindVertexArray(this.vao)
    gl.disableVertexAttribArray(8)
    gl.vertexAttrib1f(8, 0)
    gl.disableVertexAttribArray(9)
    gl.vertexAttrib1f(9, 0)
    const upload = (
      location: number,
      values: Float32Array,
      components: number,
      previous: WebGLBuffer | null,
    ) => {
      const buffer = previous || gl.createBuffer()!
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer)
      gl.bufferData(gl.ARRAY_BUFFER, values, gl.STATIC_DRAW)
      gl.enableVertexAttribArray(location)
      gl.vertexAttribPointer(location, components, gl.FLOAT, false, 0, 0)
      return buffer
    }
    const channel = (values: ArrayLike<number> | undefined) =>
      sourceIds
        ? Float32Array.from(sourceIds, (id) => values?.[id] || 0)
        : values instanceof Float32Array
          ? values
          : Float32Array.from({ length: data.count }, (_, i) => values?.[i] || 0)
    this.instCenterBuf = upload(0, centers, 3, this.instCenterBuf)
    if (this.worldFlatMode) this.instGeoBuf = upload(7, geoPositions, 3, this.instGeoBuf)
    else {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.instCenterBuf)
      gl.enableVertexAttribArray(7)
      gl.vertexAttribPointer(7, 3, gl.FLOAT, false, 0, 0)
    }
    this.instHeightBuf = upload(1, channel(data.heights), 1, this.instHeightBuf)
    const gradient = heightGradients(data.centers, data.indices, data.heights)
    const remappedGradient = sourceIds
      ? Float32Array.from(
          { length: sourceIds.length * 3 },
          (_, i) => gradient[sourceIds[Math.floor(i / 3)] * 3 + (i % 3)],
        )
      : gradient
    this.gradientBuf = upload(11, remappedGradient, 3, this.gradientBuf)
    this.instBiomeBuf = upload(2, channel(data.biomes), 1, this.instBiomeBuf)
    this.instTempBuf = upload(4, channel(data.temperatures), 1, this.instTempBuf)
    this.instPrecipBuf = upload(5, channel(data.precipitations), 1, this.instPrecipBuf)
    this.instFlowBuf = upload(6, channel(data.flows), 1, this.instFlowBuf)
    const lakeShore = lakeShoreDepths(data.heights, data.lakeDepths, data.indices)
    this.instLakeBuf = upload(10, channel(lakeShore), 1, this.instLakeBuf)
    this.instPlateBuf = upload(3, channel(data.plateIds), 1, this.instPlateBuf)
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.indexBuf)
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, indices, gl.STATIC_DRAW)
    gl.bindVertexArray(null)
    this.setCryosphere(data.cryosphere)
    this.setEcosystems(data.ecosystems, data.ecosystemMode ?? 1)
  }

  setEcosystems(data?: EcosystemData, mode = 1) {
    if (this.disposed || !this.vao) return
    const gl = this.gl
    const packed = data
      ? ecosystemAttributes(data, this.cellCount, this.geographySourceIds, mode)
      : null
    this.hasEcosystems = !!packed
    gl.bindVertexArray(this.vao)
    if (!packed) {
      for (const location of [14, 15]) {
        gl.disableVertexAttribArray(location)
        gl.vertexAttrib3f(location, 0, 0, 0)
      }
      gl.bindVertexArray(null)
      return
    }
    this.ecosystemColorBuf ||= gl.createBuffer()
    this.ecosystemPaintBuf ||= gl.createBuffer()
    for (const [buffer, location, values] of [
      [this.ecosystemColorBuf, 14, packed.colors],
      [this.ecosystemPaintBuf, 15, packed.paint],
    ] as const) {
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer)
      gl.bufferData(gl.ARRAY_BUFFER, values, gl.DYNAMIC_DRAW)
      gl.enableVertexAttribArray(location)
      gl.vertexAttribPointer(location, 3, gl.FLOAT, false, 0, 0)
    }
    gl.bindVertexArray(null)
  }

  setCryosphere(ice?: IceData) {
    if (this.disposed || !this.vao) return
    const gl = this.gl
    const packed = iceAttributes(ice, this.cellCount, this.geographySourceIds)
    gl.bindVertexArray(this.vao)
    this.iceBuf ||= gl.createBuffer()
    this.frozenBuf ||= gl.createBuffer()
    for (const [buffer, location, size, values] of [
      [this.iceBuf, 12, 4, packed.cover],
      [this.frozenBuf, 13, 2, packed.ground],
    ] as const) {
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer)
      gl.bufferData(gl.ARRAY_BUFFER, values, gl.STATIC_DRAW)
      gl.enableVertexAttribArray(location)
      gl.vertexAttribPointer(location, size, gl.FLOAT, false, 0, 0)
    }
    gl.bindVertexArray(null)
    this.hasCryosphere = !!ice
  }

  setPartitionLabels(labels: Uint32Array | null) {
    const gl = this.gl
    gl.bindVertexArray(this.vao)
    if (labels) {
      if (labels.length !== this.cellCount) throw new Error('分区标签与地图尺寸不一致')
      const values = this.geographySourceIds
        ? Float32Array.from(this.geographySourceIds, (id) => labels[id])
        : Float32Array.from(labels)
      this.partitionBuf ||= gl.createBuffer()
      gl.bindBuffer(gl.ARRAY_BUFFER, this.partitionBuf)
      gl.bufferData(gl.ARRAY_BUFFER, values, gl.STATIC_DRAW)
      gl.enableVertexAttribArray(9)
      gl.vertexAttribPointer(9, 1, gl.FLOAT, false, 0, 0)
    } else {
      gl.disableVertexAttribArray(9)
      gl.vertexAttrib1f(9, 0)
    }
    gl.bindVertexArray(null)
  }

  setGeographyMask(mask: Float32Array | null) {
    const gl = this.gl
    this.geographyActive = !!mask
    gl.bindVertexArray(this.vao)
    if (mask) {
      const values = this.geographySourceIds
        ? Float32Array.from(this.geographySourceIds, (id) => mask[id] || 0)
        : mask
      this.geographyBuf ||= gl.createBuffer()
      gl.bindBuffer(gl.ARRAY_BUFFER, this.geographyBuf)
      gl.bufferData(gl.ARRAY_BUFFER, values, gl.STATIC_DRAW)
      gl.enableVertexAttribArray(8)
      gl.vertexAttribPointer(8, 1, gl.FLOAT, false, 0, 0)
    } else {
      gl.disableVertexAttribArray(8)
      gl.vertexAttrib1f(8, 0)
    }
    gl.bindVertexArray(null)
  }

  setOverlay(lines: Float32Array) {
    const gl = this.gl
    if (this.worldFlatMode) lines = atlasLines(lines)
    const positions = this.worldFlatMode
      ? projectAtlasPositions(lines, this.worldProjection)
      : lines
    this.lineCount = lines.length / 3
    gl.bindVertexArray(this.lineVao)
    gl.bindBuffer(gl.ARRAY_BUFFER, this.lineBuf)
    gl.bufferData(gl.ARRAY_BUFFER, positions, gl.STATIC_DRAW)
    gl.enableVertexAttribArray(0)
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0)
    gl.bindBuffer(gl.ARRAY_BUFFER, this.lineGeoBuf)
    gl.bufferData(gl.ARRAY_BUFFER, lines, gl.STATIC_DRAW)
    gl.enableVertexAttribArray(1)
    gl.vertexAttribPointer(1, 3, gl.FLOAT, false, 0, 0)
    gl.bindVertexArray(null)
  }

  setDiagnostics(groups: Float32Array[]) {
    const gl = this.gl
    for (const group of this.diagnostics) {
      gl.deleteVertexArray(group.vao)
      gl.deleteBuffer(group.buffer)
      gl.deleteBuffer(group.geoBuffer)
    }
    this.diagnostics = groups.map((source) => {
      const values = this.worldFlatMode ? atlasLines(source) : source
      const positions = this.worldFlatMode
        ? projectAtlasPositions(values, this.worldProjection)
        : values
      const vao = gl.createVertexArray()!,
        buffer = gl.createBuffer()!,
        geoBuffer = gl.createBuffer()!
      gl.bindVertexArray(vao)
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer)
      gl.bufferData(gl.ARRAY_BUFFER, positions, gl.STATIC_DRAW)
      gl.enableVertexAttribArray(0)
      gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0)
      gl.bindBuffer(gl.ARRAY_BUFFER, geoBuffer)
      gl.bufferData(gl.ARRAY_BUFFER, values, gl.STATIC_DRAW)
      gl.enableVertexAttribArray(1)
      gl.vertexAttribPointer(1, 3, gl.FLOAT, false, 0, 0)
      return { vao, buffer, geoBuffer, count: values.length / 3 }
    })
    gl.bindVertexArray(null)
  }

  clear() {
    this.geographyActive = false
    this.cellCount = 0
    this.lineCount = 0
    this.setDiagnostics([])
    this.previewMode = false
  }
  resetView() {
    this.yaw = 0
    this.pitch = 0.35
    this.targetDistance = 3.2
    this.pan = [0, 0, 0]
    this.planarZoom = 1
  }
  centerAt(p: Vec3) {
    if (this.worldFlatMode) {
      const projected = projectedWorldPoint(p, this.worldProjection)
      if (!projected) return
      p = projected
    }
    if (this.planarMode) {
      this.pan = [p[0], p[1], 0]
    } else {
      const n = norm(p)
      this.yaw = Math.atan2(n[0], n[2])
      this.pitch = Math.asin(n[1])
      this.autoRotate = false
    }
  }
  measureScale(pixels = 100) {
    const r = this.canvas.getBoundingClientRect(),
      x = r.left + r.width / 2,
      y = r.top + r.height / 2,
      a = this.pick(x - pixels / 2, y),
      b = this.pick(x + pixels / 2, y)
    if (!a || !b) return null
    if (this.planarMode && !this.worldFlatMode) return Math.hypot(a[0] - b[0], a[1] - b[1])
    return 2 * Math.asin(Math.min(1, Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]) / 2))
  }
  pick(clientX: number, clientY: number): Vec3 | null {
    const rect = this.canvas.getBoundingClientRect(),
      x = ((clientX - rect.left) / rect.width) * 2 - 1,
      y = 1 - ((clientY - rect.top) / rect.height) * 2,
      aspect = rect.width / rect.height
    if (this.planarMode) {
      const half = this.planarHalfHeight / this.planarZoom
      const p: Vec3 = [this.pan[0] + x * half * aspect, this.pan[1] + y * half, 0]
      if (!this.worldFlatMode) return p
      return projectedWorldDirection(p, this.worldProjection)
    }
    const back = norm(this.eye),
      right = norm(cross([0, 1, 0], back)),
      up = cross(back, right),
      t = Math.tan(Math.PI / 8)
    const ray = norm(back.map((v, i) => -v + right[i] * x * t * aspect + up[i] * y * t) as Vec3),
      b = dot(this.eye, ray),
      disc = b * b - dot(this.eye, this.eye) + this.radius * this.radius
    if (disc < 0) return null
    const distance = -b - Math.sqrt(disc)
    if (distance < 0) return null
    return norm(this.eye.map((v, i) => v + ray[i] * distance) as Vec3)
  }
  project(p: Vec3): { x: number; y: number } | null {
    if (this.worldFlatMode) {
      const projected = projectedWorldPoint(p, this.worldProjection)
      if (!projected) return null
      p = projected
    }
    if (!this.planarMode && dot(norm(p), this.eye) < this.radius) return null
    const m = this.matrix,
      w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15]
    if (w <= 0) return null
    const x = (m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12]) / w,
      y = (m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13]) / w
    return {
      x: ((x + 1) * this.canvas.clientWidth) / 2,
      y: ((1 - y) * this.canvas.clientHeight) / 2,
    }
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
    this.disposed = true
    this.setDiagnostics([])
    this.stop()
    const gl = this.gl
    if (this.program) gl.deleteProgram(this.program)
    if (this.vao) gl.deleteVertexArray(this.vao)
    if (this.lineVao) gl.deleteVertexArray(this.lineVao)
    if (this.lineProgram) gl.deleteProgram(this.lineProgram)
    if (this.frameTexture) gl.deleteTexture(this.frameTexture)
    if (this.flowTexture) gl.deleteTexture(this.flowTexture)
    this.pigment?.dispose()
    this.pigment = null
    for (const b of [
      this.indexBuf,
      this.lineBuf,
      this.lineGeoBuf,
      this.instGeoBuf,
      this.instCenterBuf,
      this.instHeightBuf,
      this.instBiomeBuf,
      this.instTempBuf,
      this.instPrecipBuf,
      this.instFlowBuf,
      this.instLakeBuf,
      this.gradientBuf,
      this.iceBuf,
      this.frozenBuf,
      this.ecosystemColorBuf,
      this.ecosystemPaintBuf,
      this.instPlateBuf,
      this.geographyBuf,
      this.partitionBuf,
    ]) {
      if (b) gl.deleteBuffer(b)
    }
  }

  /** 拖拽旋转。 */
  rotate(dx: number, dy: number) {
    if (this.planarMode) {
      const unit = ((this.planarHalfHeight / this.planarZoom) * 2) / this.canvas.clientHeight
      this.pan[0] -= dx * unit
      this.pan[1] += dy * unit
      return
    }
    this.autoRotate = false
    this.yaw -= dx * 0.005
    this.pitch = Math.max(-1.5, Math.min(1.5, this.pitch + dy * 0.005))
  }

  /** 缩放。delta 大于 0 表示放大。 */
  zoom(delta: number) {
    if (this.planarMode) {
      this.planarZoom = Math.max(0.5, Math.min(30, this.planarZoom * Math.exp(delta * 0.001)))
      return
    }
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
      eye = [this.pan[0], this.pan[1], this.distance]
    } else {
      const cp = Math.cos(this.pitch)
      eye = [
        Math.sin(this.yaw) * cp * this.distance,
        Math.sin(this.pitch) * this.distance,
        Math.cos(this.yaw) * cp * this.distance,
      ]
    }
    this.eye = eye
    const view = lookAt(eye, this.planarMode ? this.pan : [0, 0, 0], [0, 1, 0])
    const aspect = this.canvas.width / Math.max(this.canvas.height, 1)
    // 平面模式用正交投影：格子大小严格一致，不会因透视产生远近差异
    if (this.worldFlatMode) {
      const bounds = projectionBounds(this.worldProjection)
      this.planarHalfHeight = 1.08 * Math.max(bounds.height, bounds.width / aspect)
    }
    const proj = this.planarMode
      ? orthographic(this.planarHalfHeight / this.planarZoom, aspect, -10, 10)
      : perspective(Math.PI / 4, aspect, 0.01, 100)
    const viewProj = mat4Mul(proj, view)
    this.matrix = viewProj

    gl.useProgram(this.program)
    gl.bindVertexArray(this.vao)

    const u = (name: string) => {
      if (!this.uniforms.has(name))
        this.uniforms.set(name, gl.getUniformLocation(this.program!, name))
      return this.uniforms.get(name)!
    }
    gl.uniformMatrix4fv(u('uViewProj'), false, viewProj)
    gl.uniform1f(u('uRadius'), this.radius)
    gl.uniform1f(u('uHeightScale'), this.heightScale)
    gl.uniform1f(u('uMapMetres'), this.mapMetres)
    gl.uniform1i(u('uPlanarMode'), this.planarMode ? 1 : 0)
    gl.uniform1i(u('uWorldFlatMode'), this.worldFlatMode ? 1 : 0)
    gl.uniform1i(u('uProjection'), projectionIndex(this.worldProjection))
    if (this.planarMode) {
      gl.uniform3f(u('uLightDir'), -0.45, 0.55, 0.7)
    } else {
      gl.uniform3f(u('uLightDir'), -0.4, 0.6, 0.7)
    }
    gl.uniform1i(u('uStyle'), this.style)
    gl.uniform1i(u('uLayer'), this.layer)
    gl.uniform1i(u('uGeographyActive'), this.geographyActive ? 1 : 0)
    gl.activeTexture(gl.TEXTURE2)
    gl.bindTexture(gl.TEXTURE_2D, this.pigment?.texture || null)
    gl.uniform1i(u('uPigmentTex'), 2)
    gl.activeTexture(gl.TEXTURE0)

    // 预览模式：绑定帧纹理
    gl.uniform1i(u('uPreviewMode'), this.previewMode ? 1 : 0)
    gl.uniform1i(u('uHasCryosphere'), this.hasCryosphere ? 1 : 0)
    gl.uniform1i(u('uHasEcosystems'), this.hasEcosystems ? 1 : 0)
    if (this.previewMode && this.frameTexture) {
      gl.activeTexture(gl.TEXTURE0)
      gl.bindTexture(gl.TEXTURE_2D, this.frameTexture)
      gl.uniform1i(u('uFrameTex'), 0)
      gl.activeTexture(gl.TEXTURE1)
      gl.bindTexture(gl.TEXTURE_2D, this.flowTexture)
      gl.uniform1i(u('uFlowTex'), 1)
      gl.activeTexture(gl.TEXTURE0)
      gl.uniform1f(u('uPreviewMinElev'), this.previewMinElev)
      gl.uniform1f(u('uPreviewMaxElev'), this.previewMaxElev)
      gl.uniform1f(u('uPreviewHalfHeight'), this.previewHalfHeight)
    }

    // Shared indices prevent the gaps/overlaps of independent regular hexagons.
    gl.disable(gl.CULL_FACE)
    gl.drawElements(gl.TRIANGLES, this.indexCount, gl.UNSIGNED_INT, 0)
    if ((this.lineCount || this.diagnostics.length) && this.lineProgram && !this.previewMode) {
      gl.useProgram(this.lineProgram)
      gl.bindVertexArray(this.lineVao)
      const lu = (name: string) => {
        if (!this.lineUniforms.has(name))
          this.lineUniforms.set(name, gl.getUniformLocation(this.lineProgram!, name))
        return this.lineUniforms.get(name)!
      }
      gl.uniformMatrix4fv(lu('uViewProj'), false, viewProj)
      gl.uniform1i(lu('uSphere'), this.planarMode ? 0 : 1)
      gl.uniform1i(lu('uWorldFlatMode'), this.worldFlatMode ? 1 : 0)
      gl.uniform1i(lu('uProjection'), projectionIndex(this.worldProjection))
      gl.uniform3fv(lu('uEye'), eye)
      gl.uniform1f(lu('uRadius'), this.radius)
      gl.disable(gl.DEPTH_TEST)
      gl.enable(gl.BLEND)
      gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA)
      if (![6, 7, 8, 9, 10, 11].includes(this.layer)) {
        gl.uniform4f(lu('uColor'), 0.27, 0.36, 0.29, 0.22)
        gl.drawArrays(gl.LINES, 0, this.lineCount)
      } else {
        if (this.layer === 7 && this.lineCount) {
          gl.bindVertexArray(this.lineVao)
          gl.uniform4f(lu('uColor'), 0.27, 0.36, 0.29, 0.1)
          gl.drawArrays(gl.LINES, 0, this.lineCount)
        }
        const colors =
          this.layer === 8 || this.layer === 9
            ? [
                [0.15, 0.38, 0.55, 0.9],
                [0.12, 0.51, 0.52, 0.95],
                [0.73, 0.44, 0.15, 1],
                [0.67, 0.2, 0.15, 1],
              ]
            : [
                [0.65, 0.22, 0.16, 1],
                [0.1, 0.44, 0.55, 1],
                [0.55, 0.4, 0.65, 1],
                [0.15, 0.23, 0.2, 1],
              ]
        this.diagnostics.forEach((group, index) => {
          gl.bindVertexArray(group.vao)
          gl.uniform4fv(
            lu('uColor'),
            this.layer === 10
              ? index === 1
                ? [0.34, 0.62, 0.68, 0.74]
                : [0.18, 0.46, 0.57, 0.84]
              : this.layer === 7
                ? [0.22, 0.29, 0.24, 0.8]
                : colors[index] || colors[3],
          )
          gl.drawArrays(gl.LINES, 0, group.count)
        })
      }
      gl.disable(gl.BLEND)
      gl.enable(gl.DEPTH_TEST)
    }
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
    2 / (r - l),
    0,
    0,
    0,
    0,
    2 / (tp - b),
    0,
    0,
    0,
    0,
    2 * nf,
    0,
    -(r + l) / (r - l),
    -(tp + b) / (tp - b),
    (far + near) * nf,
    1,
  ])
}
