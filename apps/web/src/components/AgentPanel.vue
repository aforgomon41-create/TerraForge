<script setup lang="ts">
/**
 * AI 助手面板。
 *
 * 三个状态：
 * 1. 未配置：让用户填提供商、模型、API key
 * 2. 已配置：对话界面
 * 3. 思考中：显示当前在调什么工具
 */
import { ref, computed, onMounted } from 'vue'
import {
  TerraAgent,
  loadConfig,
  saveConfig,
  clearConfig,
  PROVIDER_PRESETS,
} from '@/agent/core'
import { buildTools, buildSystemPrompt, type ToolContext } from '@/agent/tools'
import type { ProviderConfig, AgentMessage } from '@/agent/types'

const props = defineProps<{ toolCtx: ToolContext }>()

const cfg = ref<ProviderConfig | null>(null)
const showSettings = ref(false)
const input = ref('')
const busy = ref(false)
const statusText = ref('')
const messages = ref<{ role: string; text: string }[]>([])
const streaming = ref('')

let agent: TerraAgent | null = null

// 表单状态
const formPreset = ref('deepseek')
const formBaseUrl = ref(PROVIDER_PRESETS.deepseek.baseUrl)
const formModel = ref(PROVIDER_PRESETS.deepseek.models[0])
const formKey = ref('')

onMounted(() => {
  const saved = loadConfig()
  if (saved) {
    cfg.value = saved
    initAgent()
  } else {
    showSettings.value = true
  }
})

function onPresetChange() {
  const p = PROVIDER_PRESETS[formPreset.value]
  if (p) {
    formBaseUrl.value = p.baseUrl
    formModel.value = p.models[0] ?? ''
  }
}

function saveSettings() {
  if (!formKey.value.trim()) {
    statusText.value = '请填写 API Key'
    return
  }
  const c: ProviderConfig = {
    preset: formPreset.value,
    baseUrl: formBaseUrl.value.trim(),
    model: formModel.value.trim(),
    apiKey: formKey.value.trim(),
  }
  saveConfig(c)
  cfg.value = c
  showSettings.value = false
  initAgent()
  statusText.value = ''
}

function initAgent() {
  if (!cfg.value) return
  agent = new TerraAgent(cfg.value, buildTools(props.toolCtx), buildSystemPrompt())
}

function logout() {
  clearConfig()
  cfg.value = null
  agent = null
  messages.value = []
  showSettings.value = true
}

async function send() {
  const text = input.value.trim()
  if (!text || !agent || busy.value) return

  input.value = ''
  messages.value.push({ role: 'user', text })
  streaming.value = ''
  busy.value = true
  statusText.value = '思考中'

  await agent.send(text, {
    onText: (delta) => {
      streaming.value += delta
    },
    onTextEnd: () => {
      if (streaming.value) {
        messages.value.push({ role: 'assistant', text: streaming.value })
        streaming.value = ''
      }
    },
    onToolStart: (name) => {
      statusText.value = '调用 ' + name
    },
    onToolEnd: (name, result) => {
      const brief = JSON.stringify(result).slice(0, 80)
      messages.value.push({ role: 'tool', text: name + ': ' + brief })
      statusText.value = ''
    },
    onDone: () => {
      busy.value = false
      statusText.value = ''
    },
    onError: (msg) => {
      busy.value = false
      statusText.value = ''
      messages.value.push({ role: 'error', text: msg })
    },
  })
}

function stop() {
  agent?.abort()
  busy.value = false
}

const canSend = computed(() => input.value.trim().length > 0 && !busy.value)
</script>

<template>
  <div class="flex flex-col h-full">
    <!-- 未配置 -->
    <div v-if="showSettings" class="p-4 space-y-3 text-sm">
      <div class="text-xs text-dim/70 leading-relaxed">
        API Key 只保存在这台设备的浏览器里，
        请求直接从浏览器发往 AI 提供商，网站服务器全程不接触密钥。
      </div>

      <div>
        <label class="label-xs block mb-1">提供商</label>
        <select
          v-model="formPreset"
          class="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm
                 focus:outline-none focus:border-accent/50"
          @change="onPresetChange"
        >
          <option v-for="(p, k) in PROVIDER_PRESETS" :key="k" :value="k">{{ p.label }}</option>
        </select>
      </div>

      <div>
        <label class="label-xs block mb-1">API 地址</label>
        <input
          v-model="formBaseUrl"
          class="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm font-mono
                 focus:outline-none focus:border-accent/50"
        />
      </div>

      <div>
        <label class="label-xs block mb-1">模型</label>
        <input
          v-model="formModel"
          class="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm font-mono
                 focus:outline-none focus:border-accent/50"
        />
      </div>

      <div>
        <label class="label-xs block mb-1">API Key</label>
        <input
          v-model="formKey"
          type="password"
          placeholder="sk-..."
          class="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm font-mono
                 focus:outline-none focus:border-accent/50"
        />
      </div>

      <div v-if="statusText" class="text-xs text-danger/90">{{ statusText }}</div>

      <button class="btn-primary w-full py-2.5" @click="saveSettings">开始使用</button>
    </div>

    <!-- 对话界面 -->
    <div v-else class="flex flex-col h-full min-h-0">
      <div class="flex items-center justify-between px-3 py-2 border-b border-white/5">
        <span class="text-[11px] text-dim/60 font-mono truncate">{{ cfg?.model }}</span>
        <button class="text-[10px] text-dim/50 hover:text-ink" @click="logout">重置</button>
      </div>

      <div class="flex-1 overflow-y-auto p-3 space-y-3 min-h-0">
        <div v-if="messages.length === 0" class="text-xs text-dim/50 leading-relaxed">
          试试问：<br />
          「这颗星球有几个大洲？」<br />
          「最高的山在哪里？」<br />
          「生成一块大陆的详细地图」
        </div>

        <div
          v-for="(m, i) in messages"
          :key="i"
          class="text-xs leading-relaxed"
          :class="{
            'text-ink': m.role === 'user',
            'text-dim/80': m.role === 'assistant',
            'text-accent/70 font-mono text-[10px]': m.role === 'tool',
            'text-danger/80': m.role === 'error',
          }"
        >
          <div v-if="m.role === 'user'" class="flex justify-end">
            <span class="bg-accent/15 border border-accent/25 rounded-lg px-3 py-1.5 max-w-[85%]">
              {{ m.text }}
            </span>
          </div>
          <div v-else-if="m.role === 'tool'" class="bg-white/[0.03] rounded px-2 py-1 break-all">
            {{ m.text }}
          </div>
          <div v-else class="whitespace-pre-wrap">{{ m.text }}</div>
        </div>

        <div v-if="streaming" class="text-xs text-dim/80 whitespace-pre-wrap">{{ streaming }}</div>
        <div v-if="statusText" class="text-[10px] text-accent/60 flex items-center gap-1.5">
          <span class="spinner" style="width: 10px; height: 10px; border-width: 1.5px"></span>
          {{ statusText }}
        </div>
      </div>

      <div class="p-2 border-t border-white/5">
        <div class="flex gap-1.5">
          <input
            v-model="input"
            placeholder="问点什么..."
            class="flex-1 px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs
                   focus:outline-none focus:border-accent/50"
            @keydown.enter.exact.prevent="send"
          />
          <button v-if="busy" class="btn-ghost px-3 text-xs" @click="stop">停</button>
          <button v-else class="btn-primary px-3 text-xs" :disabled="!canSend" @click="send">
            发送
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
