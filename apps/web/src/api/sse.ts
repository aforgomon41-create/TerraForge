import { authorizedFetch } from './client'
import { decodeFrame, latestFrameQueue, type FrameEvent, type DecodedFrameEvent } from './frame'
export type { FrameEvent, FrameStats, DecodedFrameEvent } from './frame'
export interface ProgressEvent {
  job_id: string
  stage: string
  progress: number
  eta_secs: number
}
type Handlers = {
  onFrame?: (f: DecodedFrameEvent) => void
  onProgress?: (p: ProgressEvent) => void
  onStage?: (p: { to: string }) => void
  onDisconnect?: () => void
  onResync?: () => void
}
/** Fetch-based SSE supports a different Bearer capability in each browser tab. Polling is authoritative. */
export function openEventStream(jobId: string, handlers: Handlers): () => void {
  const controller = new AbortController()
  let latestTick = -1
  const frames = latestFrameQueue(
    decodeFrame,
    (frame) => {
      if (!controller.signal.aborted) handlers.onFrame?.(frame)
    },
    () => handlers.onDisconnect?.(),
  )
  void (async () => {
    let backoff = 1000
    while (!controller.signal.aborted) {
      try {
        const encoding = typeof DecompressionStream === 'undefined' ? '' : '&frame_encoding=gzip'
        const res = await authorizedFetch(
          `/api/v1/events?job_id=${encodeURIComponent(jobId)}${encoding}`,
          {
            signal: controller.signal,
          },
        )
        const reader = res.body?.getReader()
        if (!reader) throw new Error('缺少事件流')
        const decoder = new TextDecoder()
        let buffer = ''
        backoff = 1000
        try {
          while (true) {
            const { value, done } = await reader.read()
            if (done) break
            buffer += decoder.decode(value, { stream: true }).replace(/\r/g, '')
            if (buffer.length > 2_100_000) throw new Error('事件流数据过大')
            let end: number
            while ((end = buffer.indexOf('\n\n')) >= 0) {
              const block = buffer.slice(0, end)
              buffer = buffer.slice(end + 2)
              let event = 'message'
              const data: string[] = []
              for (const line of block.split('\n')) {
                if (line.startsWith('event:')) event = line.slice(6).trim()
                if (line.startsWith('data:')) data.push(line.slice(5).trimStart())
              }
              if (!data.length) continue
              const payload = JSON.parse(data.join('\n'))
              if (payload.job_id && payload.job_id !== jobId) continue
              if (event === 'done' || event === 'failed' || event === 'cancelled') {
                frames.stop()
                controller.abort()
                return
              }
              if (event === 'resync') handlers.onResync?.()
              if (event === 'frame') {
                const frame = payload as FrameEvent
                if (Number.isInteger(frame.tick) && frame.tick > latestTick) {
                  latestTick = frame.tick
                  frames.push(frame)
                }
              } else if (event === 'progress') handlers.onProgress?.(payload)
              else if (event === 'stage') handlers.onStage?.(payload)
            }
          }
        } finally {
          await reader.cancel().catch(() => {})
        }
      } catch {
        if (controller.signal.aborted) return
        handlers.onDisconnect?.()
      }
      await new Promise<void>((resolve) => {
        const done = () => {
          clearTimeout(t)
          controller.signal.removeEventListener('abort', done)
          resolve()
        }
        const t = setTimeout(done, backoff)
        controller.signal.addEventListener('abort', done, { once: true })
      })
      backoff = Math.min(backoff * 2, 8000)
    }
  })()
  return () => {
    frames.stop()
    controller.abort()
  }
}
export function decodeBase64(data: string) {
  const bin = atob(data)
  return Uint8Array.from(bin, (c) => c.charCodeAt(0))
}
