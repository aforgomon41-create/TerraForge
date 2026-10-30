/** Blank requests a fresh server seed; explicit zero remains a reproducible seed. */
export function parseSeed(value: string): number | undefined {
  const text = value.trim()
  if (!text) return undefined
  if (!/^\d+$/.test(text)) throw new Error('种子请填写非负整数，或留空自动随机')
  const seed = Number(text)
  if (!Number.isSafeInteger(seed)) throw new Error('固定种子不能超过 9007199254740991')
  return seed
}
