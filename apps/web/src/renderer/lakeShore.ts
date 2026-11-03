/** Signed distance in height to the adjacent real lake surface.
 * A dry shore vertex must be negative above water: zero would paint most of
 * the connecting triangle blue, greatly enlarging single-cell lakes.
 * Source physical depths remain unchanged; this is a rendering attribute.
 */
export function lakeShoreDepths(
  height: Float32Array,
  depth: Float32Array | undefined,
  triangles: Uint32Array,
) {
  const signed = new Float32Array(height.length)
  if (!depth) return signed
  signed.set(depth)
  const level = new Float32Array(height.length).fill(-Infinity)
  const edge = (a: number, b: number) => {
    if (depth[a] > 0 && depth[b] <= 0 && height[b] > 0)
      level[b] = Math.max(level[b], height[a] + depth[a])
    if (depth[b] > 0 && depth[a] <= 0 && height[a] > 0)
      level[a] = Math.max(level[a], height[b] + depth[b])
  }
  for (let i = 0; i < triangles.length; i += 3) {
    const a = triangles[i],
      b = triangles[i + 1],
      c = triangles[i + 2]
    edge(a, b)
    edge(b, c)
    edge(c, a)
  }
  for (let i = 0; i < signed.length; i++)
    if (Number.isFinite(level[i])) signed[i] = Math.min(0, level[i] - height[i])
  return signed
}
