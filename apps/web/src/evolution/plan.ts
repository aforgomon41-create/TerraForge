export type Process = 'uplift' | 'subsidence' | 'erosion'

export interface Footprint {
  cells: number[]
  radius_m: number
}

export interface EvolutionPlan {
  operations: { process: Process; area: Footprint; duration_years: number; intensity: number }[]
  protected_areas: Footprint[]
  strait: {
    corridor: Footprint
    minimum_width_m: number
    minimum_depth_m: number
    repair: { maximum_subsidence_m: number; duration_years: number } | null
  } | null
}

export interface Passage {
  both_ends_at_sea: boolean
  connected: boolean
  width_estimate_m: number | null
  shallowest_depth_m: number | null
  passed: boolean
}

export interface EvolutionReport {
  can_apply: boolean
  changed_cells: number
  flooded_protected_cells: number
  lost_protected_water_cells: number
  changed_lake_cells: number
  before: Passage | null
  after: Passage | null
  repair_trials: number
  repair_path: number[]
}

export function defaultEvolutionForm(spacing: number) {
  return {
    process: 'subsidence' as Process | 'goal',
    area: { cells: [] as number[], radius_m: Math.ceil(spacing * 5) },
    protectedAreas: [] as Footprint[],
    duration: 100,
    amount: 240,
    erosionStrength: 1,
    strait: false,
    corridor: { cells: [] as number[], radius_m: Math.ceil(spacing * 8) },
    minimumWidth: Math.ceil(spacing * 4),
    minimumDepth: 20,
    autoRepair: true,
    repairAmount: 240,
  }
}

export type EvolutionForm = ReturnType<typeof defaultEvolutionForm>

export function evolutionPlan(
  form: EvolutionForm,
  spacing: number,
  maxRadius: number,
): EvolutionPlan {
  const footprint = (area: Footprint, title: string) => {
    if (!area.cells.length) throw new Error(`请在地图上标记${title}`)
    if (!(area.radius_m >= spacing && area.radius_m <= maxRadius))
      throw new Error(`${title}的半径须在 ${spacing.toFixed(1)} 至 ${maxRadius.toFixed(1)} 米之间`)
    return { cells: [...area.cells], radius_m: area.radius_m }
  }
  const duration = Math.round(form.duration * 1000)
  if (!(duration >= 100 && duration <= 1_000_000))
    throw new Error('演化时间须在 0.1 至 1000 千年之间')
  const plan: EvolutionPlan = {
    operations: [],
    protected_areas: form.protectedAreas.map((area) => footprint(area, '保护区')),
    strait: null,
  }
  if (form.process !== 'goal') {
    const area = footprint(form.area, '演化范围')
    const intensity =
      form.process === 'erosion' ? form.erosionStrength : (form.amount * 1000) / duration
    if (
      !(intensity >= 0.001 && intensity <= 5) ||
      (form.process !== 'erosion' && form.amount > 3000)
    )
      throw new Error(
        form.process === 'erosion'
          ? '侵蚀强度须在 0.001 至 5 倍之间'
          : '请减小升降量或延长时间。每次最多 3000 米，每千年最多 5 米。',
      )
    plan.operations.push({ process: form.process, area, duration_years: duration, intensity })
  }
  if (form.strait || form.process === 'goal') {
    const corridor = footprint(form.corridor, '海峡搜索范围')
    if (corridor.cells.length < 2 || corridor.cells[0] === corridor.cells.at(-1))
      throw new Error('请标记两个不同的海域入口')
    if (!(form.minimumWidth >= spacing * 3 && form.minimumWidth <= corridor.radius_m * 2))
      throw new Error(`海峡宽度至少为 3 格（${Math.ceil(spacing * 3)} 米），且不能超过搜索带宽度`)
    if (!(form.minimumDepth >= 0.5 && form.minimumDepth <= 1000))
      throw new Error('最小水深须在 0.5 至 1000 米之间')
    if (
      form.autoRepair &&
      !(form.repairAmount >= 1 && form.repairAmount <= Math.min(3000, duration * 0.005))
    )
      throw new Error('请减小自动下沉上限或延长时间。自动下沉至少 1 米，每千年最多 5 米。')
    plan.strait = {
      corridor,
      minimum_width_m: form.minimumWidth,
      minimum_depth_m: form.minimumDepth,
      repair: form.autoRepair
        ? { maximum_subsidence_m: form.repairAmount, duration_years: duration }
        : null,
    }
  }
  return plan
}
