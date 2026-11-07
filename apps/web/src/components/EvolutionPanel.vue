<script setup lang="ts">
import { computed } from 'vue'
import type { EvolutionForm, EvolutionReport, Passage } from '@/evolution/plan'
import type { EvolutionPick } from '@/evolution/workbench'

const props = defineProps<{
  report: EvolutionReport | null
  hasCandidate: boolean
  readyCandidate: boolean
  showingCandidate: boolean
  terminal: boolean
  pending: boolean
  locked: boolean
  error: string
  message: string
  spacing: number
}>()
const form = defineModel<EvolutionForm>('form', { required: true })
const emit = defineEmits<{
  pick: [target: EvolutionPick]
  start: []
  compare: [candidate: boolean]
  apply: []
  discard: []
  refresh: []
  close: []
}>()
const displayStrait = computed(() => form.value.strait || form.value.process === 'goal')
const passage = computed(() =>
  props.showingCandidate ? props.report?.after : props.report?.before,
)
const lostWater = computed(() => (props.report?.lost_protected_water_cells ?? 0) > 0)
const flooded = computed(() => (props.report?.flooded_protected_cells ?? 0) > 0)
const protection = computed(() => {
  if (flooded.value && lostWater.value) return '有淹水，也有水域失去连通'
  if (flooded.value) return '保护区有淹水'
  if (lostWater.value) return '保护水域受损'
  return form.value.protectedAreas.length ? '通过' : '未设置'
})
function passageStatus(value: Passage) {
  if (!value.both_ends_at_sea) return '入口须位于海域'
  if (!value.connected) return '未连通'
  return value.passed ? '已达标' : '已连通，未达标'
}
function metres(value: number | null) {
  if (value == null) return '—'
  return value >= 1000 ? `${(value / 1000).toFixed(2)} 公里` : `${value.toFixed(1)} 米`
}
</script>

<template>
  <section class="evolution-panel" aria-labelledby="evolution-heading">
    <header class="evolution-heading">
      <div>
        <span class="eyebrow">图层与选区 / 修改工具</span>
        <h2 id="evolution-heading">区域定向演化</h2>
      </div>
      <button
        type="button"
        class="icon-button"
        aria-label="收起修改工具"
        :disabled="locked"
        @click="emit('close')"
      >
        ×
      </button>
    </header>
    <p class="muted">圈选要演化的地方，试算后比较前后差异。</p>
    <form class="evolution-form" @submit.prevent="emit('start')">
      <fieldset v-if="!hasCandidate" :disabled="locked || pending">
        <div class="evolution-row">
          <button v-if="form.process !== 'goal'" type="button" @click="emit('pick', 'area')">
            选演化范围
          </button>
          <button v-else type="button" @click="emit('pick', 'corridor')">选海峡范围</button>
          <button
            type="button"
            :disabled="form.protectedAreas.length >= 8"
            @click="emit('pick', 'protection')"
          >
            加保护区
          </button>
        </div>
        <div class="evolution-note">
          <div v-if="form.process !== 'goal'" class="evolution-row">
            <span>
              演化范围 ·
              {{ form.area.cells.length ? `${form.area.cells.length} 个标记点` : '待选' }}
            </span>
            <button v-if="form.area.cells.length" type="button" @click="form.area.cells = []">
              清除
            </button>
          </div>
          <label v-if="form.process !== 'goal' && form.area.cells.length">
            演化半径（米）
            <input
              v-model.number="form.area.radius_m"
              type="number"
              :min="spacing"
              step="any"
              required
            />
          </label>
          <span v-if="!form.protectedAreas.length">保护区 · 未设置</span>
          <div
            v-for="(area, index) in form.protectedAreas"
            :key="area.cells[0]"
            class="evolution-protection"
          >
            <label>
              保护区 {{ index + 1 }} 半径（米）
              <input
                v-model.number="area.radius_m"
                type="number"
                :min="spacing"
                step="any"
                required
              />
            </label>
            <button
              type="button"
              :aria-label="`移除保护区 ${index + 1}`"
              @click="form.protectedAreas.splice(index, 1)"
            >
              移除
            </button>
          </div>
        </div>
        <label>
          作用方式
          <select v-model="form.process">
            <option value="subsidence">下沉</option>
            <option value="uplift">抬升</option>
            <option value="erosion">侵蚀</option>
            <option value="goal">按海峡目标修正</option>
          </select>
        </label>
        <div class="evolution-row">
          <label v-if="form.process === 'erosion'">
            侵蚀强度（倍）
            <input
              v-model.number="form.erosionStrength"
              type="number"
              min="0.001"
              max="5"
              step="any"
              required
            />
          </label>
          <label v-else-if="form.process !== 'goal'">
            {{ form.process === 'uplift' ? '累计抬升' : '累计下沉' }}（米）
            <input
              v-model.number="form.amount"
              type="number"
              min="0.001"
              max="3000"
              step="any"
              required
            />
          </label>
          <label>
            演化时间（千年）
            <input
              v-model.number="form.duration"
              type="number"
              min="0.1"
              max="1000"
              step="any"
              required
            />
          </label>
        </div>
        <details :open="displayStrait">
          <summary>海峡条件</summary>
          <label v-if="form.process !== 'goal'" class="evolution-check">
            <input v-model="form.strait" type="checkbox" />
            检查海峡条件
          </label>
          <template v-if="displayStrait">
            <p class="muted">沿搜索带选点，首尾分别落在两侧海域。宽度按格网估算，至少需要 3 格。</p>
            <div class="evolution-row">
              <button type="button" @click="emit('pick', 'corridor')">
                标记海峡入口 · {{ form.corridor.cells.length }} 点
              </button>
              <button
                v-if="form.corridor.cells.length"
                type="button"
                @click="form.corridor.cells = []"
              >
                清除
              </button>
            </div>
            <label>
              搜索带半径（米）
              <input
                v-model.number="form.corridor.radius_m"
                type="number"
                :min="spacing"
                step="any"
                required
              />
            </label>
            <div class="evolution-row">
              <label>
                最小宽度（米）
                <input
                  v-model.number="form.minimumWidth"
                  type="number"
                  :min="spacing * 3"
                  step="any"
                  required
                />
              </label>
              <label>
                最小水深（米）
                <input
                  v-model.number="form.minimumDepth"
                  type="number"
                  min="0.5"
                  max="1000"
                  step="any"
                  required
                />
              </label>
            </div>
            <label class="evolution-check">
              <input v-model="form.autoRepair" type="checkbox" />
              未达标时允许自动下沉修正
            </label>
            <label v-if="form.autoRepair">
              自动下沉上限（米）
              <input
                v-model.number="form.repairAmount"
                type="number"
                min="1"
                max="3000"
                step="any"
                required
              />
            </label>
          </template>
        </details>
      </fieldset>
      <p v-if="error" class="evolution-error" role="alert">{{ error }}</p>
      <p v-if="message" role="status">{{ message }}</p>
      <button v-if="!hasCandidate" class="primary" :disabled="locked || pending" type="submit">
        开始试算
      </button>
      <button
        v-else-if="!terminal && !pending"
        type="button"
        :disabled="locked"
        @click="emit('refresh')"
      >
        刷新试算状态
      </button>
    </form>

    <div v-if="readyCandidate" class="evolution-result">
      <span class="eyebrow">试算结果</span>
      <div class="evolution-row">
        <button
          type="button"
          :aria-pressed="!showingCandidate"
          :disabled="locked || pending"
          @click="emit('compare', false)"
        >
          看原图
        </button>
        <button
          type="button"
          :aria-pressed="showingCandidate"
          :disabled="locked || pending"
          @click="emit('compare', true)"
        >
          看候选图
        </button>
      </div>
      <dl v-if="report">
        <template v-if="passage">
          <dt>海峡条件</dt>
          <dd>{{ passageStatus(passage) }}</dd>
          <dt>估算宽度 / 最浅水深</dt>
          <dd>{{ metres(passage.width_estimate_m) }} / {{ metres(passage.shallowest_depth_m) }}</dd>
        </template>
        <dt>候选图保护区</dt>
        <dd>{{ protection }}</dd>
        <dt>高程变化</dt>
        <dd>{{ report.changed_cells.toLocaleString() }} 格</dd>
        <dt>湖泊水深变化</dt>
        <dd>{{ report.changed_lake_cells.toLocaleString() }} 格</dd>
      </dl>
      <p class="muted" v-if="report?.can_apply">套用后建立新版地图，可在地图册切回原版。</p>
      <p class="muted" v-else-if="report?.changed_cells === 0 && report.after?.passed">
        海峡已符合条件，原图无需修改。
      </p>
      <p class="muted" v-else-if="report?.changed_cells === 0 && !report.after">
        当前设置未产生高程变化。
      </p>
      <p class="muted" v-else-if="report">试算未通过检查，可放弃候选图后调整参数。</p>
    </div>
    <div v-if="hasCandidate && terminal" class="evolution-row evolution-actions">
      <button type="button" :disabled="locked || pending" @click="emit('discard')">放弃试算</button>
      <button
        v-if="readyCandidate"
        class="primary"
        type="button"
        :disabled="locked || pending || !report?.can_apply"
        @click="emit('apply')"
      >
        套用为新版本
      </button>
    </div>
  </section>
</template>
