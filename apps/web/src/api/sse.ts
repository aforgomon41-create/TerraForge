/**
 * SSE 事件流客户端。
 *
 * 后端在生成过程中推送五类事件：
 * - state:    作业状态变化
 * - stage:    阶段切换
 * - progress: 进度百分比
 * - frame:    可视化帧（未压缩的原始多通道数据）
 * - done/failed: 结束
 */

export interface FrameStats {
  land_fraction: number
  max_elevation: number
  min_elevation: number
  max_temperature: number
  min_temperature: number
}

export interface FrameEvent {
  job_id: string
  tick: number
  w: number
  h: number
  fmt: string
  stats: FrameStats
  /** base64 编码的原始像素数据 */
  data: string
}

export interface ProgressEvent {
  job_id: string
  stage: string
  progress: number
  eta_secs: number
}

export interface StageEvent {
  job_id: string
  from: string
  to: string
}

export interface DoneEvent {
  job_id: string
  map_id: string
  duration_ms: number
}

type Handlers = {
  onFrame?: (f: FrameEvent) => void
  onProgress?: (p: ProgressEvent) => void
  onStage?: (s: StageEvent) => void
  onDone?: (d: DoneEvent) => void
  onFailed?: (f: { job_id: string; error: string }) => void
}

/** 打开事件流，返回关闭函数。 */
export function openEventStream(baseUrl: string, handlers: Handlers): () => void {
  const es = new EventSource(baseUrl + '/api/v1/events')

  const bind = <T>(name: string, fn?: (v: T) => void) => {
    if (!fn) return
    es.addEventListener(name, (e) => {
      try {
        fn(JSON.parse((e as MessageEvent).data))
      } catch (err) {
        console.warn('事件解析失败', name, err)
      }
    })
  }

  bind<FrameEvent>('frame', handlers.onFrame)
  bind<ProgressEvent>('progress', handlers.onProgress)
  bind<StageEvent>('stage', handlers.onStage)
  bind<DoneEvent>('done', handlers.onDone)
  bind<{ job_id: string; error: string }>('failed', handlers.onFailed)

  return () => es.close()
}

/**
 * 把 base64 解码成原始字节。
 */
export function decodeBase64(data: string): Uint8Array {
  const bin = atob(data)
  const bytes = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
  return bytes
}
