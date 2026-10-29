import { authorizedFetch } from './client'
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
  data: string
}
export interface ProgressEvent {
  job_id: string
  stage: string
  progress: number
  eta_secs: number
}
type Handlers = {
  onFrame?: (f: FrameEvent) => void
  onProgress?: (p: ProgressEvent) => void
  onStage?: (p: { to: string }) => void
  onDisconnect?: () => void
  onResync?: () => void
}
/** Fetch-based SSE supports a different Bearer capability in each browser tab. Polling is authoritative. */
export function openEventStream(jobId: string, handlers: Handlers): () => void {
  const controller = new AbortController()
  void (async () => {
    let backoff = 1000
    while (!controller.signal.aborted) {
      try {
        const res = await authorizedFetch(`/api/v1/events?job_id=${encodeURIComponent(jobId)}`, {
          signal: controller.signal,
        })
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
              if (event === 'done' || event === 'failed') {
                controller.abort()
                return
              }
              if (event === 'resync') handlers.onResync?.()
              if (event === 'frame') handlers.onFrame?.(payload)
              else if (event === 'progress') handlers.onProgress?.(payload)
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
        const t = setTimeout(resolve, backoff)
        controller.signal.addEventListener(
          'abort',
          () => {
            clearTimeout(t)
            resolve()
          },
          { once: true },
        )
      })
      backoff = Math.min(backoff * 2, 8000)
    }
  })()
  return () => controller.abort()
}
export function decodeBase64(data: string) {
  const bin = atob(data)
  return Uint8Array.from(bin, (c) => c.charCodeAt(0))
}
