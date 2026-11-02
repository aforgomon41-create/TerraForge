/** Area-weighted height gradients on the actual shared mesh (metres per map unit).
 * Work in the original globe/plane coordinates, before any atlas projection.
 * Interpolation then lights a smooth surface instead of exposing triangle faces.
 */
export function heightGradients(
  centers: Float32Array,
  indices: Uint32Array,
  heights: Float32Array,
) {
  const sums = new Float64Array(centers.length)
  const weights = new Float64Array(heights.length)
  for (let t = 0; t < indices.length; t += 3) {
    const a = indices[t],
      b = indices[t + 1],
      c = indices[t + 2]
    const ax = centers[b * 3] - centers[a * 3],
      ay = centers[b * 3 + 1] - centers[a * 3 + 1],
      az = centers[b * 3 + 2] - centers[a * 3 + 2]
    const bx = centers[c * 3] - centers[a * 3],
      by = centers[c * 3 + 1] - centers[a * 3 + 1],
      bz = centers[c * 3 + 2] - centers[a * 3 + 2]
    const nx = ay * bz - az * by,
      ny = az * bx - ax * bz,
      nz = ax * by - ay * bx
    const area = Math.hypot(nx, ny, nz)
    if (area < 1e-20) continue
    const dh1 = heights[b] - heights[a],
      dh2 = heights[c] - heights[a]
    // Area weighting cancels one factor of |edge1 × edge2| in the gradient.
    const gx = (dh1 * (by * nz - bz * ny) + dh2 * (ny * az - nz * ay)) / area,
      gy = (dh1 * (bz * nx - bx * nz) + dh2 * (nz * ax - nx * az)) / area,
      gz = (dh1 * (bx * ny - by * nx) + dh2 * (nx * ay - ny * ax)) / area
    for (let k = 0; k < 3; k++) {
      const id = indices[t + k]
      weights[id] += area
      sums[id * 3] += gx
      sums[id * 3 + 1] += gy
      sums[id * 3 + 2] += gz
    }
  }
  return Float32Array.from(sums, (sum, i) => sum / (weights[Math.floor(i / 3)] || 1))
}
