import test from 'node:test'
import assert from 'node:assert/strict'
import {
  defaults,
  scopeDefaults,
  scopeParameters,
  extentFields,
  terrainFields,
  weatherFields,
  localFields,
  planetFields,
} from '../src/generation/options.ts'

test('all initial scope controls satisfy the native number input step and bounds', () => {
  for (const scope of ['world', 'continent', 'region', 'site'] as const) {
    const values = { ...defaults, ...scopeDefaults(scope) }
    for (const fields of [extentFields, terrainFields, weatherFields, localFields, planetFields])
      for (const { key, min, max, step } of fields) {
        const value = values[key],
          ticks = (value - min) / step
        assert.ok(Number.isFinite(value) && value >= min && value <= max, `${scope}: ${key}`)
        assert.ok(
          Math.abs(ticks - Math.round(ticks)) < 1e-7,
          `${scope}: ${key}=${value} violates step=${step}`,
        )
      }
  }
  const planet = scopeParameters('world', 'sphere', defaults, 'hills', true, true).planet as Record<
    string,
    number
  >
  assert.equal(planet.axial_tilt_deg, 23.44)
})

test('a local root uses its own terrain and weather without hidden planetary or plate controls', () => {
  const values = { ...defaults, ...scopeDefaults('site'), continent_count: NaN, radius_km: NaN }
  const parameters = scopeParameters('site', 'sphere', values, 'valley', false, true)
  assert.equal(parameters.world_shape, 'plane')
  assert.equal(parameters.map_width_m, 10000)
  assert.equal(parameters.map_height_m, 7500)
  assert.equal(parameters.terrain_relief_m, 80)
  assert.equal(parameters.display_grid_m, 250)
  assert.equal(parameters.landscape, 'valley')
  assert.equal(parameters.regional_precipitation_mm, 900)
  for (const hidden of [
    'planet',
    'continent_count',
    'plate_count',
    'tectonic_age_myr',
    'land_fraction',
    'precipitation_scale',
  ])
    assert.ok(!(hidden in parameters))
})
test('sphere size and planetary fields differ from a finite independent continent', () => {
  const sphere = scopeParameters('world', 'sphere', defaults, 'hills', true, true)
  assert.equal((sphere.planet as Record<string, number>).radius_m, 6371000)
  assert.ok(!('map_width_m' in sphere))
  const continent = scopeParameters(
    'continent',
    'sphere',
    { ...defaults, ...scopeDefaults('continent') },
    'hills',
    true,
    true,
  )
  assert.equal(continent.continent_count, 1)
  assert.equal(continent.map_width_m, 6000000)
  assert.equal(continent.world_shape, 'plane')
  assert.ok(!('planet' in continent))
})
test('invalid relevant controls cannot be sent as null and display grid preserves explicit zero', () => {
  assert.throws(() =>
    scopeParameters(
      'region',
      'plane',
      { ...defaults, regional_precipitation_mm: NaN },
      'hills',
      true,
      true,
    ),
  )
  assert.throws(() =>
    scopeParameters('world', 'sphere', { ...defaults, grid_km: -1 }, 'hills', true, true),
  )
  assert.equal(
    scopeParameters('world', 'plane', { ...defaults, grid_km: 0 }, 'hills', true, true)
      .display_grid_m,
    0,
  )
})
