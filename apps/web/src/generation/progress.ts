/** Describe the solver's two clocks; this is not a remaining-time estimate. */
export function evolutionClock(elapsedMyr: number, totalMyr: number): string {
  if (!Number.isFinite(totalMyr) || totalMyr <= 0) return `构造时间 ${elapsedMyr.toFixed(1)} 百万年`
  const recent = Math.min(totalMyr, 120),
    ancient = totalMyr - recent,
    elapsed = Math.min(totalMyr, Math.max(0, elapsedMyr))
  return elapsed < ancient
    ? `远古简化演化 ${elapsed.toFixed(1)}/${ancient.toFixed(1)} 百万年`
    : `近期精细演化 ${(elapsed - ancient).toFixed(1)}/${recent.toFixed(1)} 百万年`
}
