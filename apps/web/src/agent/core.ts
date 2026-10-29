/** Browser-side OpenAI-compatible transport. Keys never go to the map server. */
import type { ToolDef, ProviderConfig, AgentCallbacks } from './types'
const STORAGE_KEY = 'tf-agent-config-v2'
let memoryConfig: ProviderConfig | null = null
export function loadConfig(): ProviderConfig | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return memoryConfig || (raw ? JSON.parse(raw) : null)
  } catch {
    return memoryConfig
  }
}
export function saveConfig(cfg: ProviderConfig, remember = false) {
  memoryConfig = { ...cfg }
  if (remember) localStorage.setItem(STORAGE_KEY, JSON.stringify(cfg))
  else localStorage.removeItem(STORAGE_KEY)
  localStorage.removeItem('tf-agent-config')
}
export function clearConfig() {
  memoryConfig = null
  localStorage.removeItem(STORAGE_KEY)
  localStorage.removeItem('tf-agent-config')
}
export function validateConfig(cfg: ProviderConfig) {
  const url = new URL(cfg.baseUrl)
  if (
    url.protocol !== 'https:' &&
    !(url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))
  )
    throw new Error('AI 地址需要 HTTPS，本机服务可以用 HTTP')
  if (url.username || url.password || url.search || url.hash)
    throw new Error('AI 地址请勿包含账号、查询参数或片段')
  if (!cfg.model.trim() || !cfg.apiKey.trim()) throw new Error('请填写模型名称和密钥')
}
interface WireCall {
  id: string
  type: 'function'
  function: { name: string; arguments: string }
}
interface ChatMessage {
  role: 'system' | 'user' | 'assistant' | 'tool'
  content: string | null
  tool_call_id?: string
  tool_calls?: WireCall[]
}
function validateArgs(
  schema: Record<string, unknown>,
  args: unknown,
): asserts args is Record<string, unknown> {
  if (!args || typeof args !== 'object' || Array.isArray(args)) throw new Error('工具参数需要对象')
  const value = args as Record<string, unknown>,
    properties = (schema.properties || {}) as Record<string, { type?: string; enum?: unknown[] }>
  for (const key of (schema.required || []) as string[])
    if (!(key in value)) throw new Error('缺少工具参数 ' + key)
  for (const [key, v] of Object.entries(value)) {
    const p = properties[key]
    if (!p) throw new Error('未知工具参数 ' + key)
    if (p.type && typeof v !== p.type) throw new Error('工具参数类型无效 ' + key)
    if (typeof v === 'number' && !Number.isFinite(v)) throw new Error('数值无效 ' + key)
    if (p.enum && !p.enum.includes(v)) throw new Error('工具参数不在允许范围 ' + key)
  }
}
export class TerraAgent {
  private messages: ChatMessage[] = []
  private controller: AbortController | null = null
  private busy = false
  constructor(
    private cfg: ProviderConfig,
    private tools: ToolDef[],
    private systemPrompt: string,
    private context: () => string = () => '',
  ) {}
  abort() {
    this.controller?.abort()
  }
  reset() {
    if (this.busy) throw new Error('请先停止回复')
    this.messages = []
  }
  history() {
    return structuredClone(this.messages)
  }
  async send(text: string, cb: AgentCallbacks): Promise<void> {
    if (this.busy) {
      cb.onError?.('已有一条回复正在进行')
      return
    }
    this.busy = true
    this.controller = new AbortController()
    const checkpoint = this.messages.length
    try {
      validateConfig(this.cfg)
      if (!text.trim() || text.length > 12000) throw new Error('问题需要 1～12000 个字符')
      this.messages.push({ role: 'user', content: text })
      for (let round = 0; round < 8; round++) {
        this.controller.signal.throwIfAborted()
        const step = await this.callLLM(cb, this.controller.signal)
        this.messages.push({
          role: 'assistant',
          content: step.text || null,
          ...(step.calls.length ? { tool_calls: step.calls } : {}),
        })
        if (!step.calls.length) {
          cb.onDone?.()
          return
        }
        for (const call of step.calls) {
          this.controller.signal.throwIfAborted()
          let output: unknown
          try {
            const tool = this.tools.find((t) => t.name === call.function.name)
            if (!tool) throw new Error('未知工具')
            const args = JSON.parse(call.function.arguments || '{}')
            validateArgs(tool.parameters, args)
            cb.onToolStart?.(tool.name, args)
            output = await tool.execute(args)
          } catch (e) {
            output = { error: e instanceof Error ? e.message : String(e) }
          }
          cb.onToolEnd?.(call.function.name, output)
          const result = JSON.stringify(output ?? null)
          this.messages.push({
            role: 'tool',
            tool_call_id: call.id,
            content:
              result.length > 24000
                ? JSON.stringify({ error: '结果过长，请缩小查询范围' })
                : result,
          })
        }
      }
      throw new Error('已达到本次 8 轮工具上限，请继续提问')
    } catch (e) {
      const message = this.controller.signal.aborted
        ? '回复已停止'
        : e instanceof Error
          ? e.message
          : String(e)
      // Keep the user's question and completed rounds. Close every advertised
      // tool call, including ones not reached before cancellation.
      if (this.messages.length > checkpoint) {
        for (let i = checkpoint; i < this.messages.length; i++) {
          const assistant = this.messages[i]
          if (!assistant.tool_calls) continue
          const results = new Set<string>()
          let j = i + 1
          while (j < this.messages.length && this.messages[j].role === 'tool') {
            results.add(this.messages[j].tool_call_id!)
            j++
          }
          const missing = assistant.tool_calls
            .filter((call) => !results.has(call.id))
            .map((call) => ({
              role: 'tool' as const,
              tool_call_id: call.id,
              content: JSON.stringify({ error: '本次回复已中断，此工具未执行' }),
            }))
          this.messages.splice(j, 0, ...missing)
          i = j + missing.length - 1
        }
        this.messages.push({ role: 'assistant', content: '[本次回复未完成：' + message + ']' })
      }
      cb.onError?.(message)
    } finally {
      this.controller = null
      this.busy = false
    }
  }
  private async callLLM(cb: AgentCallbacks, signal: AbortSignal) {
    if (JSON.stringify(this.messages).length > 160000) throw new Error('对话较长，请清空对话后继续')
    const res = await fetch(this.cfg.baseUrl.replace(/\/$/, '') + '/chat/completions', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: 'Bearer ' + this.cfg.apiKey,
      },
      body: JSON.stringify({
        model: this.cfg.model,
        messages: [
          {
            role: 'system',
            content: this.systemPrompt + '\n当前工作台状态：' + this.context(),
          },
          ...this.messages,
        ],
        stream: true,
        ...(this.tools.length
          ? {
              tools: this.tools.map((t) => ({
                type: 'function',
                function: {
                  name: t.name,
                  description: t.description,
                  parameters: t.parameters,
                },
              })),
            }
          : {}),
      }),
      signal: AbortSignal.any([signal, AbortSignal.timeout(120000)]),
    })
    if (!res.ok) throw new Error(`AI 服务返回 ${res.status}，请检查地址、模型和密钥`)
    const reader = res.body?.getReader()
    if (!reader) throw new Error('AI 响应为空')
    const decoder = new TextDecoder()
    let buffer = '',
      text = '',
      finished = false
    const calls = new Map<number, WireCall>()
    const parse = (line: string) => {
      if (!line.startsWith('data:')) return
      const data = line.slice(5).trim()
      if (!data) return
      if (data === '[DONE]') {
        finished = true
        return
      }
      const json = JSON.parse(data)
      if (json.error) throw new Error('AI 服务返回流错误')
      const choice = json.choices?.[0]
      if (!choice) return
      if (['length', 'content_filter'].includes(choice.finish_reason))
        throw new Error('AI 回复被截断，请缩短问题或调整服务端输出限制')
      if (choice.finish_reason) finished = true
      const delta = choice.delta || {}
      if (typeof delta.content === 'string') {
        text += delta.content
        if (text.length > 64000) throw new Error('AI 回复过长')
        cb.onText?.(delta.content)
      }
      for (const part of delta.tool_calls || []) {
        const index = part.index
        if (!Number.isInteger(index) || index < 0 || index > 15) throw new Error('工具调用索引无效')
        let call = calls.get(index)
        if (!call) {
          call = {
            id: '',
            type: 'function',
            function: { name: '', arguments: '' },
          }
          calls.set(index, call)
        }
        if (part.id) call.id = part.id
        if (part.function?.name) call.function.name += part.function.name
        if (part.function?.arguments) call.function.arguments += part.function.arguments
        if (call.function.arguments.length > 16000) throw new Error('工具参数过长')
      }
    }
    try {
      while (true) {
        const result = await reader.read()
        if (result.done) break
        buffer += decoder.decode(result.value, { stream: true })
        let at: number
        while ((at = buffer.indexOf('\n')) >= 0) {
          parse(buffer.slice(0, at).replace(/\r$/, ''))
          buffer = buffer.slice(at + 1)
        }
      }
      buffer += decoder.decode()
      if (buffer.trim()) parse(buffer.replace(/\r$/, ''))
      if (!finished) throw new Error('AI 连接中断，回复尚未完成')
    } finally {
      await reader.cancel().catch(() => {})
      reader.releaseLock()
    }
    const values = [...calls.entries()].sort((a, b) => a[0] - b[0]).map(([, v]) => v)
    const ids = new Set<string>()
    for (const call of values) {
      if (!call.id || !call.function.name || ids.has(call.id))
        throw new Error('AI 工具调用格式无效')
      ids.add(call.id)
    }
    cb.onTextEnd?.()
    return { text, calls: values }
  }
}
