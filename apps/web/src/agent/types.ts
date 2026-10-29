/** Agent 的类型定义。 */

export interface ProviderConfig {
  /** 提供商预设名。 */
  preset: string
  /** API 基础地址。 */
  baseUrl: string
  /** 模型 id。 */
  model: string
  /** API 密钥。默认仅本次页面使用，可由用户选择保存在设备。 */
  apiKey: string
}

export interface ToolDef {
  name: string
  description: string
  /** JSON Schema。 */
  parameters: Record<string, unknown>
  execute: (args: any) => Promise<any>
}

export type AgentMessage = {
  role: 'system' | 'user' | 'assistant' | 'toolResult'
  content: string
  toolCallId?: string
}

export interface AgentCallbacks {
  onText?: (delta: string) => void
  onTextEnd?: () => void
  onToolStart?: (name: string, args: any) => void
  onToolEnd?: (name: string, result: any) => void
  onDone?: () => void
  onError?: (msg: string) => void
}
