/**
 * TerraForge 的 Agent 工具集。
 *
 * 这些工具是 Agent 唯一能「动手」的途径，每个都对应一个后端 API，
 * 所以 Agent 的能力边界完全由这些工具定义。
 */

import type { MapNode } from '@/api/client'
import type { ToolDef } from './types'

/** 工具执行时需要的上下文。 */
export interface ToolContext {
  worldId: () => string | null
  mapId: () => string | null
  maps: () => MapNode[]
  elevation: () => Float32Array | null
  biome: () => Uint8Array | null
  generate: () => Promise<void>
  drillDown: (parentId: string, kind: string) => Promise<void>
  selectMap: (id: string) => Promise<void>
  setLayer: (layer: string) => void
  setStyle: (style: string) => void
}

/** 构建工具集。 */
export function buildTools(ctx: ToolContext): ToolDef[] {
  return [
    {
      name: 'world_summary',
      description:
        '获取当前世界的整体概况：世界 id、格数、陆地占比、高程范围、地图树。回答任何关于世界状态的问题前应先调用。',
      parameters: { type: 'object', properties: {} },
      execute: async () => {
        const elev = ctx.elevation()
        if (!elev) return { error: '还没有加载任何地图数据' }

        let land = 0
        let minE = Infinity
        let maxE = -Infinity
        let sumE = 0
        for (let i = 0; i < elev.length; i++) {
          const e = elev[i]
          if (e > 0) land++
          if (e < minE) minE = e
          if (e > maxE) maxE = e
          sumE += e
        }

        return {
          world_id: ctx.worldId(),
          current_map_id: ctx.mapId(),
          cells: elev.length,
          land_fraction: +(land / elev.length).toFixed(3),
          elevation: {
            min_m: Math.round(minE),
            max_m: Math.round(maxE),
            mean_m: Math.round(sumE / elev.length),
          },
          maps: ctx.maps().map((m) => ({
            id: m.id,
            kind: m.kind,
            cells: m.cells,
            status: m.status,
          })),
        }
      },
    },

    {
      name: 'terrain_stats',
      description:
        '统计当前地图的地形分布：陆地占比、高程分位数、生物群系占比。用于回答「这颗星球长什么样」。',
      parameters: { type: 'object', properties: {} },
      execute: async () => {
        const elev = ctx.elevation()
        const biome = ctx.biome()
        if (!elev) return { error: '没有地图数据' }

        const sorted = Array.from(elev).sort((a, b) => a - b)
        const pct = (p: number) => Math.round(sorted[Math.floor((sorted.length - 1) * p)])

        const biomeNames = [
          '海洋', '冰盖', '苔原', '针叶林', '温带阔叶林', '温带草原', '地中海',
          '沙漠', '稀树草原', '热带雨林', '热带季雨林', '高山草甸', '湿地',
        ]
        let biomeCounts: Record<string, number> | null = null
        if (biome) {
          const counts = new Array(13).fill(0)
          for (let i = 0; i < biome.length; i++) {
            const b = biome[i]
            if (b < 13) counts[b]++
          }
          biomeCounts = {}
          for (let i = 0; i < 13; i++) {
            if (counts[i] > 0) {
              biomeCounts[biomeNames[i]] = +((counts[i] / biome.length) * 100).toFixed(1)
            }
          }
        }

        return {
          cells: elev.length,
          elevation_percentiles_m: {
            p0: pct(0),
            p25: pct(0.25),
            p50: pct(0.5),
            p75: pct(0.75),
            p90: pct(0.9),
            p99: pct(0.99),
            p100: pct(1),
          },
          biome_percent: biomeCounts,
        }
      },
    },

    {
      name: 'list_maps',
      description: '列出当前世界的地图树，包括每张图的层级、格数、生成状态。',
      parameters: { type: 'object', properties: {} },
      execute: async () => ({
        maps: ctx.maps().map((m) => ({
          id: m.id,
          kind: m.kind,
          parent: m.parent,
          cells: m.cells,
          cell_size_m: m.cell_size_m,
          status: m.status,
        })),
      }),
    },

    {
      name: 'generate_world',
      description: '触发当前世界的世界层生成。跑完整物理模拟，耗时几秒到几十秒。',
      parameters: { type: 'object', properties: {} },
      execute: async () => {
        await ctx.generate()
        return { ok: true, message: '世界生成完成' }
      },
    },

    {
      name: 'drill_down',
      description:
        '在指定地图节点下生成更细的一层。kind 可选 continent、region、site。',
      parameters: {
        type: 'object',
        properties: {
          parent_id: { type: 'string', description: '父地图 id' },
          kind: {
            type: 'string',
            enum: ['continent', 'region', 'site'],
            description: '子图层级',
          },
        },
        required: ['parent_id', 'kind'],
      },
      execute: async (args: { parent_id: string; kind: string }) => {
        await ctx.drillDown(args.parent_id, args.kind)
        return { ok: true, message: '已生成 ' + args.kind + ' 层子图' }
      },
    },

    {
      name: 'select_map',
      description: '切换当前显示的地图。',
      parameters: {
        type: 'object',
        properties: {
          map_id: { type: 'string', description: '要切换到的地图 id' },
        },
        required: ['map_id'],
      },
      execute: async (args: { map_id: string }) => {
        await ctx.selectMap(args.map_id)
        return { ok: true }
      },
    },

    {
      name: 'set_style',
      description:
        '切换地图视觉风格：realistic（写实）、watercolor（水彩）、parchment（羊皮纸）、contour（等高线）。',
      parameters: {
        type: 'object',
        properties: {
          style: { type: 'string', enum: ['realistic', 'watercolor', 'parchment', 'contour'] },
        },
        required: ['style'],
      },
      execute: async (args: { style: string }) => {
        ctx.setStyle(args.style)
        return { ok: true, style: args.style }
      },
    },
  ]
}

/** 系统提示词。 */
export function buildSystemPrompt(): string {
  return [
    '你是 TerraForge 世界生成器的 AI 助手。',
    '',
    '你的职责是帮用户创造和探索一颗虚构的星球。这颗星球的地形不是随机噪声刷出来的，',
    '而是通过真实的地球物理过程演化的：地幔对流驱动板块漂移、板块碰撞抬高山脉、',
    '河流侵蚀切出峡谷、山脉阻挡水汽形成雨影、雨影决定沙漠的位置。',
    '',
    '# 你能做什么',
    '',
    '你可以调用工具来观测世界状态、触发生成、切换视图。',
    '但你不能直接修改地形——那是后端物理引擎的职责。',
    '',
    '# 回答风格',
    '',
    '- 简洁直接，不要冗长的铺垫',
    '- 谈到具体数值时说清楚单位（米、毫米每年、摄氏度）',
    '- 如果用户问「为什么这座山在这里」，从板块运动的角度解释',
    '- 不确定的事情直说不确定，不要编造',
    '',
    '# 重要约束',
    '',
    '- 生成世界需要几秒到几十秒，触发后要告诉用户等待',
    '- 每次只做一件事，不要连续触发多个耗时操作',
    '- 用户没有要求时不要主动生成或修改任何东西',
  ].join('\n')
}
