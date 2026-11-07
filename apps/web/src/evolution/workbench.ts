import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api, type EvolutionPreview, type JobStatus, type MapNode } from '@/api/client'
import { useWorldStore } from '@/stores/world'
import { defaultEvolutionForm, evolutionPlan, type EvolutionPlan } from './plan'

export type EvolutionPick = 'area' | 'protection' | 'corridor'

export function useDirectedEvolution(store: ReturnType<typeof useWorldStore>) {
  const sourceId = ref<string | null>(null)
  const candidateId = ref<string | null>(null)
  const form = ref(defaultEvolutionForm(1))
  const preview = ref<EvolutionPreview | null>(null)
  const job = ref<JobStatus | null>(null)
  const pick = ref<EvolutionPick | null>(null)
  const working = ref(false)
  const polling = ref(false)
  const error = ref('')
  const message = ref('')
  let reader: AbortController | undefined

  const eligibility = computed(() => {
    const map = store.currentMap
    if (!map || map.status !== 'done' || !map.grid) return null
    if (map.grid.type === 'sphere') return '请先从球形地图细化一张局部图。'
    if (map.cells > 512 * 512) return '请细化一张不超过 262,144 个采样点的局部图。'
    return ''
  })
  const spacing = computed(() => store.currentMap?.cell_size_m ?? 1)
  const maxRadius = computed(() => {
    const grid = store.currentMap?.grid
    if (!grid || grid.type === 'sphere') return spacing.value
    return Math.hypot(
      grid.width_m,
      grid.width_m * (grid.type === 'plane' ? grid.rows / grid.size : 1),
    )
  })
  const report = computed(() => preview.value?.evolution?.report ?? null)
  const readyCandidate = computed(() => preview.value?.map.status === 'done')
  const showingCandidate = computed(
    () => !!candidateId.value && store.currentMap?.id === candidateId.value,
  )
  const terminal = computed(() =>
    ['done', 'failed', 'cancelled'].includes(job.value?.state ?? preview.value?.map.status ?? ''),
  )

  function stopReading() {
    reader?.abort()
    reader = undefined
    polling.value = false
  }

  function clear() {
    stopReading()
    sourceId.value = candidateId.value = null
    preview.value = job.value = pick.value = null
    error.value = ''
    message.value = ''
  }

  watch(
    () => store.currentMap?.id,
    (id) => {
      if (sourceId.value && id !== sourceId.value && id !== candidateId.value) clear()
    },
  )
  onBeforeUnmount(clear)

  async function action(fn: () => Promise<unknown>) {
    if (working.value) return
    working.value = true
    error.value = ''
    try {
      await fn()
    } catch (e) {
      if ((e as Error).name !== 'AbortError') error.value = (e as Error).message
    } finally {
      working.value = false
    }
  }

  function restore(plan: EvolutionPlan) {
    const op = plan.operations[0]
    const goal = plan.strait
    const defaults = defaultEvolutionForm(spacing.value)
    form.value = {
      ...defaults,
      process: op?.process ?? 'goal',
      area: op?.area ?? defaults.area,
      protectedAreas: plan.protected_areas,
      duration: (op?.duration_years ?? goal?.repair?.duration_years ?? 100_000) / 1000,
      amount: op && op.process !== 'erosion' ? (op.duration_years * op.intensity) / 1000 : 240,
      erosionStrength: op?.process === 'erosion' ? op.intensity : 1,
      strait: !!goal,
      corridor: goal?.corridor ?? defaults.corridor,
      minimumWidth: goal?.minimum_width_m ?? defaults.minimumWidth,
      minimumDepth: goal?.minimum_depth_m ?? 20,
      autoRepair: !!goal?.repair,
      repairAmount: goal?.repair?.maximum_subsidence_m ?? 240,
    }
  }

  async function readPreview(id: string, signal: AbortSignal) {
    const result = await api.getEvolution(id, signal)
    if (signal.aborted) return
    preview.value = result
    candidateId.value = result?.map.id ?? null
    job.value = result?.job ?? null
    if (result?.evolution) restore(result.evolution.plan)
  }

  async function follow(show: boolean) {
    const source = sourceId.value
    if (!source) return
    stopReading()
    const controller = new AbortController()
    reader = controller
    const { signal } = controller
    polling.value = true
    try {
      if (!job.value || terminal.value) await readPreview(source, signal)
      if (job.value?.state === 'done' && !readyCandidate.value) await readPreview(source, signal)
      const deadline = Date.now() + 30 * 60_000
      while (job.value && !terminal.value && !signal.aborted) {
        const result = await api.getJob(job.value.id, signal)
        if (signal.aborted) return
        job.value = result
        if (terminal.value) {
          await readPreview(source, signal)
          break
        }
        if (Date.now() > deadline) throw new Error('试算仍在运行，请刷新状态。')
        await new Promise((resolve) => setTimeout(resolve, 900))
      }
      if (signal.aborted) return
      if (job.value?.state === 'failed' || job.value?.state === 'cancelled') {
        message.value =
          job.value.error || (job.value.state === 'cancelled' ? '试算已取消。' : '试算未完成。')
      } else if (
        show &&
        readyCandidate.value &&
        candidateId.value &&
        store.currentMap?.id === source
      ) {
        await store.loadMapData(candidateId.value)
      }
    } catch (e) {
      if (!signal.aborted) error.value = (e as Error).message
    } finally {
      if (reader === controller) {
        reader = undefined
        polling.value = false
      }
    }
  }

  function open(map: MapNode) {
    clear()
    sourceId.value = map.revision?.draft ? map.revision.source_id : map.id
    form.value = defaultEvolutionForm(map.cell_size_m)
    void follow(false)
  }

  function start() {
    return action(async () => {
      if (!sourceId.value || candidateId.value || polling.value) return
      const source = sourceId.value
      const plan = evolutionPlan(form.value, spacing.value, maxRadius.value)
      message.value = ''
      pick.value = null
      const { map_id, job_id } = await api.previewEvolution(source, plan)
      if (sourceId.value !== source) return
      candidateId.value = map_id
      job.value = { id: job_id, map_id, state: 'queued', stage: '排队', progress: 0, error: null }
      void follow(true)
    })
  }

  function refresh() {
    error.value = ''
    void follow(false)
  }

  function cancel() {
    return action(async () => {
      if (!job.value || terminal.value) return
      const id = job.value.id
      const result = await api.cancelJob(id)
      if (result.ok && job.value?.id === id && !terminal.value) job.value.state = 'cancelling'
      if (!polling.value) refresh()
    })
  }

  function compare(candidate: boolean) {
    return action(async () => {
      const id = candidate ? (readyCandidate.value ? candidateId.value : null) : sourceId.value
      if (id && store.currentMap?.id !== id) await store.loadMapData(id)
    })
  }

  function close() {
    return action(async () => {
      if (showingCandidate.value && sourceId.value) await store.loadMapData(sourceId.value)
      clear()
    })
  }

  function discard() {
    return action(async () => {
      if (!candidateId.value || !sourceId.value || polling.value || !terminal.value) return
      if (showingCandidate.value) await store.loadMapData(sourceId.value)
      await api.discardEvolution(candidateId.value)
      candidateId.value = null
      preview.value = job.value = null
      message.value = ''
    })
  }

  function apply() {
    return action(async () => {
      if (!candidateId.value || !report.value?.can_apply) return
      const result = await api.applyEvolution(candidateId.value)
      if (!store.maps.some((map) => map.id === result.id)) store.maps = [...store.maps, result]
      if (store.currentMap?.id === result.id) store.currentMap = result
      else await store.loadMapData(result.id)
      clear()
    })
  }

  function select(cell: number) {
    if (!pick.value || candidateId.value || store.currentMap?.id !== sourceId.value) return false
    const f = form.value
    if (pick.value === 'protection') {
      if (f.protectedAreas.length < 8 && !f.protectedAreas.some((area) => area.cells[0] === cell))
        f.protectedAreas.push({ cells: [cell], radius_m: Math.ceil(spacing.value * 2) })
      pick.value = null
    } else {
      const area = pick.value === 'area' ? f.area : f.corridor
      if (area.cells.length < 12 && area.cells.at(-1) !== cell) area.cells.push(cell)
    }
    return true
  }

  return {
    sourceId,
    candidateId,
    form,
    report,
    job,
    pick,
    error,
    message,
    eligibility,
    spacing,
    maxRadius,
    readyCandidate,
    showingCandidate,
    terminal,
    working,
    polling,
    open,
    start,
    refresh,
    cancel,
    compare,
    close,
    discard,
    apply,
    select,
  }
}
