<script setup lang="ts">
import { computed, ref } from 'vue'
import type { MapNode } from '@/api/client'
const props = defineProps<{
  maps: MapNode[]
  currentId: string | null
  disabled?: boolean
}>()
const emit = defineEmits<{ select: [id: string] }>()
const collapsed = ref(new Set<string>())
const names: Record<string, string> = {
  world: '世界',
  continent: '大陆',
  region: '区域',
  site: '地点',
}
const states: Record<string, string> = {
  pending: '等待',
  queued: '排队',
  running: '生成中',
  done: '可查看',
  failed: '失败',
  cancelled: '已取消',
  cancelling: '取消中',
}
const rows = computed(() => {
  const result: { node: MapNode; depth: number; children: boolean }[] = [],
    seen = new Set<string>()
  function walk(parent: string | null, depth: number) {
    for (const node of props.maps.filter((m) => m.parent === parent)) {
      if (seen.has(node.id)) continue
      seen.add(node.id)
      const children = props.maps.some((m) => m.parent === node.id)
      result.push({ node, depth, children })
      if (!collapsed.value.has(node.id)) walk(node.id, depth + 1)
    }
  }
  walk(null, 0)
  return result
})
function toggle(id: string) {
  const set = new Set(collapsed.value)
  if (set.has(id)) set.delete(id)
  else set.add(id)
  collapsed.value = set
}
</script>
<template>
  <div class="map-tree" role="tree" aria-label="地图层级">
    <p v-if="!rows.length" class="empty-note">
      这里将收录你的第一张地图。
      <br />
      从“创建”开始，让世界成形。
    </p>
    <div
      v-for="{ node, depth, children } in rows"
      :key="node.id"
      class="tree-row"
      :class="{ selected: currentId === node.id }"
      :style="{ '--depth': depth }"
    >
      <button
        v-if="children"
        class="tree-toggle"
        :aria-label="collapsed.has(node.id) ? '展开子图' : '收起子图'"
        :aria-expanded="!collapsed.has(node.id)"
        @click="toggle(node.id)"
      >
        {{ collapsed.has(node.id) ? '›' : '⌄' }}
      </button>
      <span v-else class="tree-leaf">{{ depth ? '└' : '◎' }}</span>
      <button
        class="tree-item"
        role="treeitem"
        :aria-selected="currentId === node.id"
        :aria-level="depth + 1"
        :disabled="disabled || node.status !== 'done'"
        @click="emit('select', node.id)"
      >
        <span>
          {{ node.name || names[node.kind] }}
          <small>
            {{
              node.revision ? '演化版本' : node.parent ? '精细图' : names[node.kind] + '范围地图'
            }}
          </small>
        </span>
        <span class="tree-meta">
          <i :class="node.status" />
          {{ states[node.status] || node.status }} · {{ node.cells.toLocaleString() }} 格
        </span>
      </button>
    </div>
  </div>
</template>
