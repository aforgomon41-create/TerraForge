/** An optional paint material. A complete neutral texture exists before image IO. */
export function createPigmentTexture(
  gl: WebGL2RenderingContext,
  url: string,
  makeImage: () => HTMLImageElement = () => new Image(),
) {
  const texture = gl.createTexture()
  if (!texture) throw new Error('地图材质纹理未能创建')
  let disposed = false
  const image = makeImage()
  const bind = (upload: () => void) => {
    const previous = gl.getParameter(gl.ACTIVE_TEXTURE) as number
    gl.activeTexture(gl.TEXTURE2)
    gl.bindTexture(gl.TEXTURE_2D, texture)
    try {
      upload()
    } finally {
      gl.activeTexture(previous)
    }
  }
  bind(() => {
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
    // Mirroring joins even imperfectly tileable pigment edges without a color jump.
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.MIRRORED_REPEAT)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.MIRRORED_REPEAT)
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA,
      1,
      1,
      0,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      new Uint8Array([141, 141, 141, 255]),
    )
    gl.generateMipmap(gl.TEXTURE_2D)
  })
  image.onload = () => {
    if (disposed) return
    bind(() => {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, image)
      gl.generateMipmap(gl.TEXTURE_2D)
    })
    image.onload = image.onerror = null
  }
  // Keep the neutral fallback when the optional material cannot load.
  image.onerror = () => {
    image.onload = image.onerror = null
  }
  image.src = url
  return {
    texture,
    dispose() {
      if (disposed) return
      disposed = true
      image.onload = image.onerror = null
      gl.deleteTexture(texture)
    },
  }
}
