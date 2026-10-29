/**
 * TerraForge 的浏览器端 AI 助手。
 *
 * # 设计原则
 *
 * 1. **密钥永不经过服务器**
 *    API key 存在 localStorage，只在这里用于直连 LLM 提供商。
 *    后端从头到尾看不到它。
 *
 * 2. **Agent 跑在浏览器里**
 *    没有 Node sidecar，没有服务端代理。
 *
 * 3. **工具执行走本地 API**
 *    Agent 调用的工具都是对后端的 HTTP 请求，
 *    能力边界完全由这些工具定义。
 *
 * # 为什么直接用 fetch 而不是 pi-ai
 *
 * pi-ai 的 provider 是固定预设，不支持任意自定义 baseUrl
 * （而我们要支持用户填任何 OpenAI 兼容端点）。
 * OpenAI 兼容的 chat completions 协议本身很简单，
 * 直接实现约 100 行，比适配 pi-ai 的内部结构更可控。
 */

import type { ToolDef, ProviderConfig, AgentCallbacks } from './types'

const STORAGE_KEY = 'tf-agent-config'

export function loadConfig(): ProviderConfig | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return raw ? JSON.parse(raw) : null
  } catch {
    return null
  }
}

export function saveConfig(cfg: ProviderConfig) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(cfg))
}

export function clearConfig() {
  localStorage.removeItem(STORAGE_KEY)
}

/** 常用提供商预设。全部走 OpenAI 兼容协议。 */
export const PROVIDER_PRESETS: Record<
  string,
  { label: string; baseUrl: string; models: string[] }
> = {
  deepseek: {
    label: 'DeepSeek',
    baseUrl: 'https://api.deepseek.com/v1',
    models: ['deepseek-chat', 'deepseek-reasoner'],
  },
  openai: {
    label: 'OpenAI',
    baseUrl: 'https://api.openai.com/v1',
    models: ['gpt-4o', 'gpt-4o-mini'],
  },
  openrouter: {
    label: 'OpenRouter',
    baseUrl: 'https://openrouter.ai/api/v1',
    models: ['anthropic/claude-sonnet-4', 'google/gemini-2.5-pro'],
  },
  moonshot: {
    label: 'Moonshot',
    baseUrl: 'https://api.moonshot.cn/v1',
    models: ['moonshot-v1-32k'],
  },
  zhipu: {
    label: '智谱',
    baseUrl: 'https://open.bigmodel.cn/api/paas/v4',
    models: ['glm-4-plus'],
  },
  custom: {
    label: '自定义',
    baseUrl: '',
    models: [],
  },
}

/** OpenAI 兼容的 chat 消息。 */
interface ChatMessage {
  role: 'system' | 'user' | 'assistant' | 'tool'
  content: string
  tool_call_id?: string
}

interface ToolCall {
  id: string
  name: string
  args: any
}

/**
 * Agent 运行时。
 */
export class TerraAgent {
  private cfg: ProviderConfig
  private tools: ToolDef[]
  private systemPrompt: string
  private messages: ChatMessage[] = []
  private abortFlag = false

  constructor(cfg: ProviderConfig, tools: ToolDef[], systemPrompt: string) {
    this.cfg = cfg
    this.tools = tools
    this.systemPrompt = systemPrompt
  }

  abort() {
    this.abortFlag = true
  }

  reset() {
    this.messages = []
  }

  history(): ChatMessage[] {
    return this.messages
  }

  /**
   * 发一条消息并跑完整 agent 循环。退出条件是模型不再调用工具。
   */
  async send(userText: string, cb: AgentCallbacks): Promise<void> {
    this.abortFlag = false
    this.messages.push({ role: 'user', content: userText })

    const MAX_ROUNDS = 8
    for (let round = 0; round < MAX_ROUNDS; round++) {
      if (this.abortFlag) {
        cb.onError?.('已中断')
        return
      }

      const step = await this.callLLM(cb)
      if (!step) return

      if (step.text) {
        this.messages.push({ role: 'assistant', content: step.text })
      }

      if (step.toolCalls.length === 0) {
        cb.onDone?.()
        return
      }

      // 把 assistant 的 tool_calls 也加进历史，否则后续请求会 400
      this.messages[this.messages.length - 1] = {
        role: 'assistant',
        content: step.text,
      }

      for (const call of step.toolCalls) {
        if (this.abortFlag) {
          cb.onError?.('已中断')
          return
        }

        cb.onToolStart?.(call.name, call.args)

        let content: string
        try {
          const tool = this.tools.find((t) => t.name === call.name)
          if (!tool) throw new Error('未知工具: ' + call.name)
          const out = await tool.execute(call.args)
          content = JSON.stringify(out)
          cb.onToolEnd?.(call.name, out)
        } catch (e) {
          const msg = e instanceof Error ? e.message : String(e)
          content = JSON.stringify({ error: msg })
          cb.onToolEnd?.(call.name, { error: msg })
        }

        this.messages.push({
          role: 'tool',
          tool_call_id: call.id,
          content,
        })
      }
    }

    cb.onError?.('超过最大工具调用轮数')
  }

  /** 调一次 LLM。 */
  private async callLLM(
    cb: AgentCallbacks
  ): Promise<{ text: string; toolCalls: ToolCall[] } | null> {
    const body: any = {
      model: this.cfg.model,
      messages: [
        { role: 'system', content: this.systemPrompt },
        ...this.messages,
      ],
      stream: true,
      temperature: 0.7,
    }

    if (this.tools.length > 0) {
      body.tools = this.tools.map((t) => ({
        type: 'function',
        function: {
          name: t.name,
          description: t.description,
          parameters: t.parameters,
        },
      }))
    }

    let res: Response
    try {
      res = await fetch(this.cfg.baseUrl.replace(/\/$/, '') + '/chat/completions', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: 'Bearer ' + this.cfg.apiKey,
        },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(120_000),
      })
    } catch (e) {
      cb.onError?.('网络请求失败：' + (e instanceof Error ? e.message : String(e)))
      return null
    }

    if (!res.ok) {
      const text = await res.text().catch(() => '')
      cb.onError?.('API 错误 ' + res.status + ': ' + text.slice(0, 200))
      return null
    }

    // 解析 SSE 流
    let fullText = ''
    const toolCalls: ToolCall[] = []
    const toolAcc: Record<number, { id: string; name: string; args: string }> = {}

    const reader = res.body?.getReader()
    if (!reader) {
      cb.onError?.('响应没有 body')
      return null
    }

    const decoder = new TextDecoder()
    let buffer = ''

    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      if (this.abortFlag) {
        reader.cancel()
        break
      }

      buffer += decoder.decode(value, { stream: true })
      const lines = buffer.split('\n')
      buffer = lines.pop() ?? ''

      for (const line of lines) {
        const trimmed = line.trim()
        if (!trimmed.startsWith('data:')) continue
        const payload = trimmed.slice(5).trim()
        if (payload === '[DONE]') continue

        let chunk: any
        try {
          chunk = JSON.parse(payload)
        } catch {
          continue
        }

        const delta = chunk.choices?.[0]?.delta
        if (!delta) continue

        if (delta.content) {
          fullText += delta.content
          cb.onText?.(delta.content)
        }

        // 工具调用按 index 累积
        if (delta.tool_calls) {
          for (const tc of delta.tool_calls) {
            const idx = tc.index ?? 0
            if (!toolAcc[idx]) {
              toolAcc[idx] = { id: '', name: '', args: '' }
            }
            if (tc.id) toolAcc[idx].id = tc.id
            if (tc.function?.name) toolAcc[idx].name += tc.function.name
            if (tc.function?.arguments) toolAcc[idx].args += tc.function.arguments
          }
        }
      }
    }

    for (const acc of Object.values(toolAcc)) {
      if (!acc.name) continue
      let args = {}
      try {
        args = JSON.parse(acc.args || '{}')
      } catch {
        args = {}
      }
      toolCalls.push({ id: acc.id || 'call_' + toolCalls.length, name: acc.name, args })
    }

    cb.onTextEnd?.()
    return { text: fullText, toolCalls }
  }
}
