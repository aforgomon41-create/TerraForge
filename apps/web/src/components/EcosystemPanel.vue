<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ecosystemCatalogue, type EcosystemResponse } from '@/ecosystems/data'
import { ecosystemModes, ecosystemDistribution, supportLabels } from '@/ecosystems/view'
const props = defineProps<{
  data: EcosystemResponse | null
  mode: number
  generating: boolean
  hasMap: boolean
}>()
const emit = defineEmits<{ (e: 'update:mode', value: number): void }>()
const expanded = ref(false)
const rows = computed(() =>
  props.data?.available ? ecosystemDistribution(props.data, props.mode) : [],
)
const visibleRows = computed(() => (expanded.value ? rows.value : rows.value.slice(0, 8)))
const present = computed(() => {
  const counts = new Map<number, number>()
  if (!props.generating && props.data?.available)
    for (const values of [props.data.group, props.data.habitatGroup])
      for (const id of values) if (id) counts.set(id, (counts.get(id) || 0) + 1)
  return counts
})
const families = ecosystemCatalogue.biomes.map((b) => ({
  ...b,
  groups: ecosystemCatalogue.groups.filter((g) => g.biome_id === b.id),
}))
const status = computed(() =>
  props.generating
    ? '最终气候与水文完成后识别；当前为真实地形预览。'
    : !props.hasMap
      ? '生成或打开地图后显示生态分类。'
      : props.data?.available
        ? '按背景生态着色，河流等局部生境在选区另列。'
        : props.data?.reason || '正在读取生态资料…',
)
watch(
  () => props.mode,
  () => (expanded.value = false),
)
watch(
  () => props.data,
  () => (expanded.value = false),
)
</script>
<template>
  <section class="ecosystem-panel" aria-label="生态分类">
    <div class="eco-modes" role="group" aria-label="生态分类层级">
      <button
        v-for="item in ecosystemModes"
        :key="item.id"
        :aria-pressed="mode === item.id"
        :class="{ active: mode === item.id }"
        :disabled="generating || !data?.available"
        @click="emit('update:mode', item.id)"
      >
        {{ item.name }}
      </button>
    </div>
    <p class="eco-help" role="status">{{ status }}</p>
    <div v-if="data?.available && !generating" class="eco-legend" aria-label="当前地图生态图例">
      <p class="eco-caption">当前地图 · {{ rows.length }} 项 · 采样点占比</p>
      <div v-for="row in visibleRows" :key="row.id" class="eco-row">
        <i :style="{ background: row.color }" aria-hidden="true" />
        <span>
          {{ row.name }}
          <small v-if="row.environment">{{ row.environment }}</small>
        </span>
        <small>{{ row.percent < 0.1 ? '＜0.1%' : `${row.percent.toFixed(1)}%` }}</small>
      </div>
      <button
        v-if="rows.length > 8"
        class="eco-expand"
        :aria-expanded="expanded"
        @click="expanded = !expanded"
      >
        {{ expanded ? '收起图例' : `查看全部 ${rows.length} 项` }}
      </button>
    </div>
    <details class="eco-catalogue">
      <summary>
        完整生态名录 · {{ families.length }}大类 / {{ ecosystemCatalogue.groups.length }}细类
      </summary>
      <p class="eco-help">名录包含本阶段尚不能自动识别的类型。条件不足时保留大类，细类显示待定。</p>
      <details v-for="family in families" :key="family.id" class="eco-family">
        <summary>
          {{ family.name_zh }}
          <small>{{ family.code }}</small>
        </summary>
        <div v-for="g in family.groups" :key="g.id" class="eco-entry">
          <span>
            {{ g.name_zh }}
            <small>{{ g.code }}</small>
          </span>
          <small :class="{ supported: g.support === 'environment_estimate' }">
            {{ supportLabels[g.support] || '需补充条件' }}
          </small>
          <small v-if="present.has(g.id)" class="eco-present">本图出现 · 背景或局部生境</small>
        </div>
      </details>
    </details>
  </section>
</template>
<style scoped>
.ecosystem-panel {
  margin: 14px 0;
  color: inherit;
  font-size: 13px;
}
.eco-modes {
  display: flex;
  padding: 4px;
  background: #edeee4;
  border-radius: 9px;
}
.eco-modes button {
  flex: 1;
  border: 0;
  background: transparent;
  border-radius: 6px;
  padding: 9px 3px;
  font: inherit;
  color: inherit;
  cursor: pointer;
}
.eco-modes button.active {
  background: #fffdf6;
  box-shadow: 0 1px 4px #33482e15;
}
.eco-modes button:disabled {
  opacity: 0.5;
  cursor: default;
}
.eco-modes button:focus-visible,
summary:focus-visible,
.eco-expand:focus-visible {
  outline: 2px solid #557766;
  outline-offset: 3px;
}
.eco-help {
  font-size: 12px;
  line-height: 1.7;
  color: #6e7769;
  margin: 12px 0;
}
.eco-caption {
  font-size: 11px;
  color: #6e7769;
}
.eco-row {
  display: grid;
  grid-template-columns: 11px minmax(0, 1fr) auto;
  align-items: start;
  gap: 9px;
  padding: 8px 0;
  border-bottom: 1px solid #e5e3d8;
}
.eco-row i {
  height: 11px;
  border-radius: 2px;
  margin-top: 4px;
}
.eco-row small {
  font-size: 11px;
  color: #6e7769;
}
.eco-row span small {
  display: block;
  margin-top: 3px;
  line-height: 1.5;
}
.eco-expand {
  border: 0;
  background: transparent;
  padding: 10px 0;
  color: #476957;
  font: inherit;
  cursor: pointer;
}
.eco-catalogue {
  border-top: 1px solid #d8ddce;
  padding-top: 13px;
  margin-top: 14px;
}
summary {
  cursor: pointer;
  line-height: 1.6;
}
.eco-family {
  margin: 12px 0;
}
.eco-family > summary {
  font-size: 12px;
}
.eco-family > summary small {
  color: #6e7769;
  font-size: 10px;
}
.eco-entry {
  display: flex;
  flex-wrap: wrap;
  align-items: start;
  gap: 6px;
  padding: 10px 0 10px 12px;
  border-bottom: 1px solid #e5e3d8;
}
.eco-entry > span {
  flex: 1;
  min-width: 110px;
  font-size: 12px;
}
.eco-entry span small {
  display: block;
  font-size: 10px;
  color: #7b8274;
  margin-top: 3px;
}
.eco-entry > small {
  font-size: 10px;
  background: #eeede4;
  color: #746d60;
  padding: 3px 6px;
  border-radius: 4px;
  max-width: 120px;
}
.eco-entry > small.supported {
  background: #e6eddf;
  color: #486344;
}
.eco-entry > small.eco-present {
  background: transparent;
  color: #486344;
  padding: 0;
  max-width: none;
  width: 100%;
}
</style>
