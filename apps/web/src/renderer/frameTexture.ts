/** Keep both elevation bytes: reducing them to 8 bits makes coasts and relief jump. */
export function frameTextureChannels(data: Uint8Array, width: number, height: number) {
  const count = width * height
  if (data.length !== count * 5) throw new Error('预览帧尺寸无效')
  const rgba = new Uint8Array(count * 4),
    flow = new Uint8Array(count)
  for (let i = 0; i < count; i++) {
    for (let channel = 0; channel < 4; channel++) rgba[i * 4 + channel] = data[i * 5 + channel]
    flow[i] = data[i * 5 + 4]
  }
  return { rgba, flow }
}
