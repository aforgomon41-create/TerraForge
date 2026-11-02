export type StartScope = 'world' | 'continent' | 'region' | 'site'
export interface ParameterField {
  key: string
  label: string
  min: number
  max: number
  step: number
}
const field = (
  key: string,
  label: string,
  min: number,
  max: number,
  step = 0.1,
): ParameterField => ({ key, label, min, max, step })
export const defaults: Record<string, number> = {
  width_km: 8000,
  height_km: 5000,
  latitude: 25,
  longitude: 0,
  grid_km: 100,
  continent_count: 4,
  continent_scale: 1,
  coast_complexity: 0.35,
  minor_plate_fraction: 0.55,
  tectonic_activity: 1,
  hotspot_count: 8,
  spreading_rate: 1,
  orogeny_strength: 1,
  rift_strength: 1,
  shear_strength: 1,
  interior_relief: 1,
  precipitation_scale: 1,
  wind_speed_m_s: 10,
  orographic_strength: 1,
  moisture_recycling: 0.35,
  base_elevation_m: 500,
  terrain_relief_m: 600,
  regional_temperature_c: 15,
  regional_precipitation_mm: 900,
  radius_km: 6371,
  axial_tilt_deg: 23.44,
  rotation_period_hours: 24,
  solar_constant: 1361,
  atmosphere_density: 1,
  greenhouse_factor: 1,
}
export const extentFields = [
  field('width_km', '覆盖宽度 · km', 0.004, 200000, 0.001),
  field('height_km', '覆盖高度 · km', 0.004, 100000, 0.001),
  field('latitude', '参考纬度 · °', -85, 85),
  field('longitude', '参考经度 · °', -180, 180),
]
export const terrainFields = [
  field('continent_count', '主要大陆数量 · 目标', 1, 12, 1),
  field('continent_scale', '大陆延展程度', 0.3, 3),
  field('coast_complexity', '海岸复杂度', 0, 1, 0.05),
  field('minor_plate_fraction', '初始小板块比例', 0.2, 0.8, 0.05),
  field('tectonic_activity', '构造活动强度', 0.2, 2),
  field('hotspot_count', '热点数量', 0, 20, 1),
  field('spreading_rate', '海底扩张倍率', 0.1, 3),
  field('orogeny_strength', '造山强度', 0, 3),
  field('rift_strength', '裂谷强度', 0, 3),
  field('shear_strength', '走滑强度', 0, 3),
  field('interior_relief', '内陆起伏', 0, 3),
]
export const weatherFields = [
  field('precipitation_scale', '降水倍率', 0.3, 2),
  field('wind_speed_m_s', '盛行风速 · m/s', 1, 40),
  field('orographic_strength', '地形雨强度', 0, 3),
  field('moisture_recycling', '水汽再循环比例', 0, 0.8, 0.05),
]
export const localFields = [
  field('base_elevation_m', '基准海拔 · m', -1000, 8000, 10),
  field('terrain_relief_m', '地形起伏 · m', 0, 6000, 10),
  field('regional_temperature_c', '基准年均温 · ℃', -60, 50),
  field('regional_precipitation_mm', '平均年降水 · mm', 0, 12000, 10),
  field('orographic_strength', '地形雨强度', 0, 3),
]
export const planetFields = [
  field('radius_km', '星球半径 · km', 1000, 50000, 1),
  field('axial_tilt_deg', '轴倾角 · °', 0, 90),
  field('rotation_period_hours', '自转周期 · 小时', 1, 1000),
  field('solar_constant', '恒星辐射 · W/m²', 100, 4000, 1),
  field('atmosphere_density', '大气密度倍率', 0.05, 10, 0.05),
  field('greenhouse_factor', '温室效应倍率', 0.1, 3),
]
export function scopeDefaults(scope: StartScope): Record<string, number> {
  if (scope === 'continent')
    return { width_km: 6000, height_km: 4000, grid_km: 50, continent_count: 1 }
  if (scope === 'region')
    return {
      width_km: 200,
      height_km: 150,
      grid_km: 5,
      base_elevation_m: 500,
      terrain_relief_m: 600,
    }
  if (scope === 'site')
    return {
      width_km: 10,
      height_km: 7.5,
      grid_km: 0.25,
      base_elevation_m: 150,
      terrain_relief_m: 80,
    }
  return { ...defaults }
}
export function scopeParameters(
  scope: StartScope,
  shape: 'sphere' | 'plane',
  values: Record<string, number>,
  landscape: string,
  monsoon: boolean,
  glacier: boolean,
): Record<string, unknown> {
  const local = scope === 'region' || scope === 'site'
  const parameters: Record<string, unknown> = {
    start_level: scope,
    world_shape: scope === 'world' ? shape : 'plane',
    glacier_enabled: glacier,
    display_grid_m: values.grid_km * 1000,
  }
  const copy = (fields: ParameterField[]) =>
    fields.forEach(({ key, label, min, max }) => {
      const value = values[key]
      if (!Number.isFinite(value) || value < min || value > max)
        throw new Error(`${label} 应在 ${min}～${max} 之间`)
      parameters[key] = value
    })
  if (scope !== 'world' || shape === 'plane') {
    copy(extentFields)
    parameters.map_width_m = values.width_km * 1000
    parameters.map_height_m = values.height_km * 1000
    parameters.center_lat_deg = values.latitude
    parameters.center_lon_deg = values.longitude
    for (const key of ['width_km', 'height_km', 'latitude', 'longitude']) delete parameters[key]
  }
  if (local) {
    copy(localFields)
    parameters.landscape = landscape
  } else {
    copy(terrainFields)
    copy(weatherFields)
    parameters.monsoon_enabled = monsoon
  }
  if (scope === 'world') {
    const planet: Record<string, number> = {}
    for (const { key, label, min, max } of planetFields) {
      const value = values[key]
      if (!Number.isFinite(value) || value < min || value > max)
        throw new Error(`${label} 超出范围`)
      planet[key === 'radius_km' ? 'radius_m' : key] = key === 'radius_km' ? value * 1000 : value
    }
    parameters.planet = planet
  }
  if (!Number.isFinite(values.grid_km) || values.grid_km < 0 || values.grid_km > 10000)
    throw new Error('显示格距应在 0～10000 km 之间')
  return parameters
}
