import assert from 'node:assert/strict'
import test from 'node:test'
import { defaultEvolutionForm, evolutionPlan } from '../src/evolution/plan.ts'

test('manual subsidence retains the selected path and sends a rate in metres per thousand years', () => {
  const form = defaultEvolutionForm(125)
  form.area.cells = [43, 51, 62]
  form.amount = 60
  form.duration = 30
  form.protectedAreas.push({ cells: [49], radius_m: 300 })

  const plan = evolutionPlan(form, 125, 6000)
  assert.deepEqual(plan.operations, [
    {
      process: 'subsidence',
      area: { cells: [43, 51, 62], radius_m: 625 },
      duration_years: 30_000,
      intensity: 2,
    },
  ])
  assert.deepEqual(plan.protected_areas, [{ cells: [49], radius_m: 300 }])
  assert.equal(plan.strait, null)
  form.area.cells.push(69)
  assert.deepEqual(plan.operations[0].area.cells, [43, 51, 62])
})

test('a strait-only trial validates gate order and width at the source grid scale', () => {
  const form = defaultEvolutionForm(500)
  form.process = 'goal'
  form.corridor.cells = [212, 240, 212]
  assert.throws(() => evolutionPlan(form, 500, 30_000), /两个不同/)
  form.corridor.cells[2] = 255
  form.minimumWidth = 1499
  assert.throws(() => evolutionPlan(form, 500, 30_000), /至少为 3 格/)
  form.minimumWidth = 1500
  const result = evolutionPlan(form, 500, 30_000)
  assert.deepEqual(result.operations, [])
  assert.deepEqual(result.strait?.corridor.cells, [212, 240, 255])
  assert.equal(result.strait?.repair?.duration_years, 100_000)
  form.autoRepair = false
  assert.equal(evolutionPlan(form, 500, 30_000).strait?.repair, null)
})
