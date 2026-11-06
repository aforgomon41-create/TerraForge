<script setup lang="ts">
import type { RefinementPlan } from '@/api/client'
defineProps<{
  source: string
  kind: string
  feature: string | null
  scope: 'full' | 'custom'
  precision: 'auto' | 'fixed'
  width: number
  spacing: number
  name: string
  plan: RefinementPlan | null
  loading: boolean
  error: string
  locked: boolean
}>()
const emit = defineEmits<{
  'update:scope': [value: 'full' | 'custom']
  'update:precision': [value: 'auto' | 'fixed']
  'update:width': [value: number]
  'update:spacing': [value: number]
  'update:name': [value: string]
  submit: []
}>()
</script>

<template>
  <form class="refine-form" @submit.prevent="emit('submit')">
    <p class="refine-source">
      来源：{{ source }}
      <br />
      <small v-if="feature">已识别：{{ feature }}</small>
    </p>
    <div class="refine-scope" role="group" aria-label="生成范围">
      <button
        type="button"
        :aria-pressed="scope === 'full'"
        :disabled="locked || !feature"
        @click="emit('update:scope', 'full')"
      >
        完整对象
      </button>
      <button
        type="button"
        :aria-pressed="scope === 'custom'"
        :disabled="locked"
        @click="emit('update:scope', 'custom')"
      >
        自定范围
      </button>
    </div>
    <p v-if="!feature" class="hint">此处没有对应的地理对象，可自定范围细化。</p>
    <label>
      地图名称
      <input
        :value="name"
        maxlength="80"
        :disabled="locked"
        @input="emit('update:name', ($event.target as HTMLInputElement).value)"
      />
    </label>
    <label>
      覆盖宽度（km）
      <input
        :value="scope === 'full' ? (plan ? plan.width_m / 1000 : '') : width"
        type="number"
        min="0.004"
        step="any"
        required
        :readonly="scope === 'full'"
        :disabled="locked"
        @input="emit('update:width', ($event.target as HTMLInputElement).valueAsNumber)"
      />
    </label>
    <label>
      计算精度
      <select
        :value="precision"
        :disabled="locked"
        @change="
          emit('update:precision', ($event.target as HTMLSelectElement).value as 'auto' | 'fixed')
        "
      >
        <option value="auto">自动匹配范围与资源预算</option>
        <option value="fixed">指定格距 · 精细局部</option>
      </select>
    </label>
    <label v-if="precision === 'fixed'">
      指定计算格距（m）
      <input
        :value="spacing"
        type="number"
        min="1"
        step="any"
        required
        :disabled="locked"
        @input="emit('update:spacing', ($event.target as HTMLInputElement).valueAsNumber)"
      />
    </label>
    <div class="refine-summary" aria-live="polite" :aria-busy="loading">
      <template v-if="loading">正在核验范围与精度…</template>
      <template v-else-if="error">
        <span role="alert">{{ error }}</span>
      </template>
      <template v-else-if="plan">
        <template v-if="plan.can_generate">
          本次：{{ plan.side }} × {{ plan.side }} 采样点
          <br />
          计算格距约
          {{
            plan.cell_size_m >= 1000
              ? (plan.cell_size_m / 1000).toLocaleString(undefined, { maximumFractionDigits: 2 }) +
                ' km'
              : plan.cell_size_m.toLocaleString(undefined, { maximumFractionDigits: 2 }) + ' m'
          }}
          <br />
          <small>显示网格另行设置</small>
        </template>
        <span v-else>本次范围或指定精度无法生成</span>
      </template>
    </div>
    <p v-if="plan && !loading" class="hint">{{ plan.message }}</p>
    <button class="primary" :disabled="locked || loading || !plan?.can_generate || !!error">
      生成{{ kind }}{{ scope === 'full' ? '概览' : '精细图' }} →
    </button>
  </form>
</template>
