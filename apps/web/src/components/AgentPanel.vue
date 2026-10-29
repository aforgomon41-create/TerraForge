<script setup lang="ts">
import { ref, nextTick, onBeforeUnmount } from 'vue'
import { TerraAgent, loadConfig, saveConfig, clearConfig, validateConfig } from '@/agent/core'
import { buildTools, buildSystemPrompt, type ToolContext } from '@/agent/tools'
import type { ProviderConfig } from '@/agent/types'
const props = defineProps<{ toolCtx: ToolContext }>()
const emit = defineEmits<{ close: [] }>()
const loaded = loadConfig(),
  settings = ref(!loaded),
  busy = ref(false),
  question = ref(''),
  status = ref(''),
  remember = ref(false),
  log = ref<HTMLElement | null>(null)
const form = ref<ProviderConfig>(loaded || { preset: 'custom', baseUrl: '', model: '', apiKey: '' })
const messages = ref<{ role: string; text: string }[]>([])
let agent: TerraAgent | null = null
function init() {
  agent = new TerraAgent({ ...form.value }, buildTools(props.toolCtx), buildSystemPrompt(), () =>
    JSON.stringify({
      world: props.toolCtx.worldId(),
      map: props.toolCtx.mapId(),
      selection: props.toolCtx.selection(),
    }),
  )
}
if (loaded) init()
function save() {
  try {
    validateConfig(form.value)
    saveConfig(form.value, remember.value)
    init()
    settings.value = false
    status.value = ''
  } catch (e) {
    status.value = String(e)
  }
}
function forget() {
  agent?.abort()
  clearConfig()
  form.value.apiKey = ''
  agent = null
  settings.value = true
  messages.value = []
  status.value = '已清除已保存的密钥'
}
async function scroll() {
  await nextTick()
  log.value?.scrollTo({ top: log.value.scrollHeight })
}
async function send(text = question.value) {
  if (busy.value || !agent || !text.trim()) return
  question.value = ''
  messages.value.push({ role: 'user', text })
  const reply = { role: 'assistant', text: '' }
  messages.value.push(reply)
  const index = messages.value.length - 1
  busy.value = true
  status.value = '正在查看这颗世界…'
  void scroll()
  try {
    await agent.send(text, {
      onText: (delta) => {
        messages.value[index].text += delta
        void scroll()
      },
      onToolStart: (name) => {
        status.value = '正在查询：' + name
      },
      onToolEnd: (name) => {
        status.value = '已完成：' + name
      },
      onError: (message) => {
        messages.value.push({ role: 'error', text: message })
      },
      onDone: () => {
        status.value = ''
      },
    })
  } finally {
    busy.value = false
    status.value = ''
    void scroll()
  }
}
function reset() {
  if (busy.value) return
  agent?.reset()
  messages.value = []
}
onBeforeUnmount(() => agent?.abort())
</script>
<template>
  <aside class="paper-panel agent-panel" aria-label="AI 地理助手">
    <header class="panel-heading">
      <div>
        <span class="eyebrow">GEOGRAPHY COMPANION</span>
        <h2>与世界对话</h2>
      </div>
      <button class="icon-button" aria-label="关闭 AI 助手" @click="emit('close')">×</button>
    </header>
    <form v-if="settings" class="agent-settings" @submit.prevent="save">
      <p class="muted">连接你自己的 AI。填写兼容 OpenAI Chat Completions 的地址和模型名称。</p>
      <label>
        服务地址
        <input
          v-model="form.baseUrl"
          type="url"
          placeholder="https://你的服务地址/v1"
          required
          spellcheck="false"
        />
      </label>
      <label>
        模型名称
        <input
          v-model="form.model"
          placeholder="填写服务商提供的模型 ID"
          required
          spellcheck="false"
        />
      </label>
      <label>
        API 密钥
        <input
          v-model="form.apiKey"
          type="password"
          autocomplete="off"
          placeholder="仅用于浏览器直连 AI 服务"
          required
        />
      </label>
      <label class="check-label">
        <input v-model="remember" type="checkbox" />
        在此设备记住密钥
      </label>
      <p class="hint">
        默认只在当前页面使用。网站后端不接收密钥；AI
        请求会把问题和所查询的地图数据发给你填写的服务。
      </p>
      <button class="primary" type="submit">连接并开始对话 ↗</button>
      <button type="button" class="text-button" @click="forget">清除已保存配置</button>
      <p v-if="status" role="alert" class="error-text">{{ status }}</p>
    </form>
    <template v-else>
      <div class="agent-toolbar">
        <span class="mono">{{ form.model }}</span>
        <button :disabled="busy" @click="settings = true">设置</button>
        <button :disabled="busy" @click="reset">清空</button>
      </div>
      <div ref="log" class="chat-log" role="log" aria-label="对话记录">
        <div v-if="!messages.length" class="chat-welcome">
          <span class="compass-mark">✧</span>
          <h3>从一个地理问题开始。</h3>
          <p>
            读懂一片河谷，或为下一次探索
            <br />
            找到一个落点。
          </p>
          <button @click="send('请查询当前地图，介绍它的地形与生态。')">
            这颗世界有哪些地貌？ ↗
          </button>
          <button @click="send('请查询我选中的位置，介绍当地气候，并区分数据与成因推断。')">
            解读我选中的位置 ↗
          </button>
        </div>
        <div v-for="(m, i) in messages" :key="i" class="chat-message" :class="m.role">
          <small>{{ m.role === 'user' ? '你' : m.role === 'error' ? '提示' : '地理助手' }}</small>
          <p>{{ m.text || '…' }}</p>
        </div>
      </div>
      <form class="chat-compose" @submit.prevent="send()">
        <span class="hint" role="status">
          {{ status || '基于当前地图回答，成因解释仅作近似参考。' }}
        </span>
        <textarea
          v-model="question"
          aria-label="地理问题"
          placeholder="这片山地为什么多雨？"
          rows="3"
          maxlength="12000"
          @keydown.ctrl.enter.prevent="send()"
        />
        <div>
          <small>Ctrl + Enter 发送</small>
          <button v-if="busy" type="button" @click="agent?.abort()">停止回复</button>
          <button v-else type="submit" class="primary" :disabled="!question.trim()">发送 ↗</button>
        </div>
      </form>
    </template>
  </aside>
</template>
