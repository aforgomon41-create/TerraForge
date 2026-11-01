<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { featureKinds, type Geography, type GeographicFeature } from '@/geography/catalog'
const props = defineProps<{
  geography: Geography | null
  loading: boolean
  error: string
  selected: number | null
  disabled?: boolean
}>()
const emit = defineEmits<{ select: [id: number] }>()
const expanded = ref(new Set<number>()),
  limits = ref(new Map<number, number>())
watch(
  () => props.geography,
  () => {
    expanded.value = new Set()
    limits.value = new Map()
  },
)
const groups = computed(() => {
  const map = new Map<number, GeographicFeature[]>()
  for (const f of props.geography?.features || []) {
    const parent = f.parent || 0,
      list = map.get(parent) || []
    list.push(f)
    map.set(parent, list)
  }
  return map
})
const summary = computed(() => {
  const counts = new Map<string, number>()
  for (const f of props.geography?.features || []) counts.set(f.kind, (counts.get(f.kind) || 0) + 1)
  return ['continent', 'island', 'land_fragment', 'drainage_region']
    .filter((k) => counts.has(k))
    .map((k) => `${counts.get(k)} ${featureKinds[k]}`)
    .join(' · ')
})
const rows = computed(() => {
  const result: { id: number; depth: number; more?: boolean }[] = []
  function walk(parent: number, depth: number) {
    const list = groups.value.get(parent) || [],
      limit = limits.value.get(parent) || 20
    for (const f of list.slice(0, limit)) {
      result.push({ id: f.id, depth })
      if (expanded.value.has(f.id)) walk(f.id, depth + 1)
    }
    if (list.length > limit) result.push({ id: parent, depth, more: true })
  }
  walk(0, 0)
  return result
})
function toggle(id: number) {
  const set = new Set(expanded.value)
  if (set.has(id)) set.delete(id)
  else set.add(id)
  expanded.value = set
}
function more(id: number) {
  limits.value = new Map(limits.value).set(id, (limits.value.get(id) || 20) + 20)
}
</script>
<template>
  <section class="feature-tree" aria-label="自动识别的地理对象" aria-live="polite">
    <h3>
      地理目录
      <small>自动识别</small>
    </h3>
    <p v-if="loading">正在识别地形与汇水关系…</p>
    <p v-else-if="error" class="error">识别暂未完成：{{ error }}</p>
    <template v-else-if="geography">
      <p>{{ summary || '当前地图没有陆地' }}</p>
      <div
        v-for="row in rows"
        :key="`${row.more ? 'more' : 'item'}-${row.id}`"
        class="feature-row"
        :style="{ paddingLeft: `${row.depth * 12}px` }"
      >
        <button v-if="row.more" class="more" @click="more(row.id)">再显示 20 项…</button>
        <template v-else>
          <button
            v-if="groups.has(row.id)"
            class="toggle"
            :aria-expanded="expanded.has(row.id)"
            :aria-label="`${expanded.has(row.id) ? '收起' : '展开'}${geography.features[row.id - 1].name}`"
            @click="toggle(row.id)"
          >
            {{ expanded.has(row.id) ? '⌄' : '›' }}
          </button>
          <span v-else class="toggle">·</span>
          <button
            class="feature"
            :class="{ active: selected === row.id }"
            :disabled="disabled"
            @click="emit('select', row.id)"
          >
            <span>{{ geography.features[row.id - 1].name }}</span>
            <small>
              {{ featureKinds[geography.features[row.id - 1].kind]
              }}{{ geography.features[row.id - 1].partial ? ' · 范围未完整' : '' }}
            </small>
          </button>
        </template>
      </div>
      <p class="hint">展开陆块查看汇水区域，再展开区域查看显著地点。湖盆表示地形洼地。</p>
    </template>
  </section>
</template>
<style scoped>
.feature-tree {
  border-top: 1px solid var(--line, #d9d7ca);
  margin-top: 16px;
  padding-top: 12px;
}
h3 {
  margin: 0;
  font-size: 16px;
}
h3 small {
  font-size: 11px;
  font-weight: normal;
  margin-left: 8px;
}
p {
  font-size: 12px;
  line-height: 1.6;
  opacity: 0.8;
  margin: 8px 0;
}
.feature-row {
  display: flex;
  align-items: center;
  min-height: 38px;
}
.toggle {
  width: 24px;
  flex-shrink: 0;
  text-align: center;
  padding: 4px;
  border: 0;
  background: transparent;
  color: inherit;
}
.feature {
  flex: 1;
  min-width: 0;
  text-align: left;
  border: 0;
  background: transparent;
  padding: 7px;
  border-radius: 6px;
  color: inherit;
  display: flex;
  flex-direction: column;
}
.feature small {
  font-size: 10px;
  opacity: 0.65;
  margin-top: 2px;
}
.feature:hover,
.feature.active {
  background: #dce6dc;
}
.more {
  border: 0;
  background: transparent;
  font-size: 12px;
  padding: 8px;
  color: inherit;
}
.error {
  color: #9b4531;
}
</style>
