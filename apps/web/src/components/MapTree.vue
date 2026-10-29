<script setup lang="ts">
/**
 * 地图树面板。
 *
 * 展示世界 -> 大陆 -> 区域 -> 地点的层级结构，
 * 用户可以在这里选择要生成的子图、跳转到已生成的图。
 */
import { computed } from 'vue'
import type { MapNode } from '@/api/client'

const props = defineProps<{
  maps: MapNode[]
  currentId: string | null
}>()

const emit = defineEmits<{
  (e: 'select', id: string): void
  (e: 'generate', parent: MapNode, kind: string): void
}>()

const LEVEL_NAMES: Record<string, string> = {
  world: '世界',
  continent: '大陆',
  region: '区域',
  site: '地点',
}

const NEXT_LEVEL: Record<string, string | null> = {
  world: 'continent',
  continent: 'region',
  region: 'site',
  site: null,
}

const STATUS_LABEL: Record<string, string> = {
  pending: '待生成',
  running: '生成中',
  done: '已完成',
  failed: '失败',
}

/** 按父子关系组织成树。 */
const tree = computed(() => {
  const byParent = new Map<string | null, MapNode[]>()
  for (const m of props.maps) {
    const key = m.parent
    if (!byParent.has(key)) byParent.set(key, [])
    byParent.get(key)!.push(m)
  }
  // 根是无父节点的
  const roots = byParent.get(null) ?? []
  // 按创建时间排序
  for (const list of byParent.values()) {
    list.sort((a, b) => a.created_at - b.created_at)
  }
  return { byParent, roots }
})

function childrenOf(id: string): MapNode[] {
  return tree.value.byParent.get(id) ?? []
}

function canGenerate(m: MapNode): boolean {
  return m.status === 'done' && NEXT_LEVEL[m.kind] !== null
}
</script>

<template>
  <div class="flex flex-col gap-1 text-sm">
    <div v-if="tree.roots.length === 0" class="text-dim/50 text-xs p-3">
      还没有地图
    </div>

    <template v-for="root in tree.roots" :key="root.id">
      <!-- 根节点：世界 -->
      <div
        class="group rounded-lg px-3 py-2 cursor-pointer transition-colors"
        :class="currentId === root.id
          ? 'bg-accent/15 border border-accent/30'
          : 'hover:bg-white/5 border border-transparent'"
        @click="emit('select', root.id)"
      >
        <div class="flex items-center justify-between gap-2">
          <div class="flex items-center gap-2 min-w-0">
            <span class="text-[10px] px-1.5 py-0.5 rounded bg-white/10 text-dim/80 shrink-0">
              {{ LEVEL_NAMES[root.kind] || root.kind }}
            </span>
            <span class="truncate">{{ root.cells.toLocaleString() }} 格</span>
          </div>
          <button
            v-if="canGenerate(root)"
            class="opacity-0 group-hover:opacity-100 transition-opacity text-[10px]
                   px-1.5 py-0.5 rounded bg-accent/20 text-accent hover:bg-accent/30 shrink-0"
            @click.stop="emit('generate', root, NEXT_LEVEL[root.kind]!)"
          >
            + 下钻
          </button>
        </div>
      </div>

      <!-- 子节点 -->
      <div
        v-for="child in childrenOf(root.id)"
        :key="child.id"
        class="group ml-4 rounded-lg px-3 py-2 cursor-pointer transition-colors"
        :class="currentId === child.id
          ? 'bg-accent/15 border border-accent/30'
          : 'hover:bg-white/5 border border-transparent'"
        @click="emit('select', child.id)"
      >
        <div class="flex items-center justify-between gap-2">
          <div class="flex items-center gap-2 min-w-0">
            <span class="text-[10px] px-1.5 py-0.5 rounded bg-white/10 text-dim/80 shrink-0">
              {{ LEVEL_NAMES[child.kind] || child.kind }}
            </span>
            <span class="truncate text-xs">{{ child.cells.toLocaleString() }} 格</span>
            <span
              v-if="child.status !== 'done'"
              class="text-[10px] text-accent/80 shrink-0"
            >
              {{ STATUS_LABEL[child.status] }}
            </span>
          </div>
          <button
            v-if="canGenerate(child)"
            class="opacity-0 group-hover:opacity-100 transition-opacity text-[10px]
                   px-1.5 py-0.5 rounded bg-accent/20 text-accent hover:bg-accent/30 shrink-0"
            @click.stop="emit('generate', child, NEXT_LEVEL[child.kind]!)"
          >
            + 下钻
          </button>
        </div>
      </div>
    </template>
  </div>
</template>
