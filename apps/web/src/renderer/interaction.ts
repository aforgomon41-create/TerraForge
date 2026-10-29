import type { HexSphereRenderer } from './HexSphereRenderer'
import type { Vec3 } from './math'
/** Pointer, pinch and keyboard input share one disposable lifetime. */
export function bindMapInput(
  canvas: HTMLCanvasElement,
  renderer: HexSphereRenderer,
  onPick: (p: Vec3) => void,
  onMove: () => void,
) {
  const controller = new AbortController(),
    signal = controller.signal
  const pointers = new Map<
    number,
    { x: number; y: number; startX: number; startY: number; moved: boolean }
  >()
  canvas.addEventListener(
    'pointerdown',
    (e) => {
      pointers.set(e.pointerId, {
        x: e.clientX,
        y: e.clientY,
        startX: e.clientX,
        startY: e.clientY,
        moved: pointers.size > 0,
      })
      if (pointers.size > 1) for (const p of pointers.values()) p.moved = true
      canvas.setPointerCapture(e.pointerId)
    },
    { signal },
  )
  canvas.addEventListener(
    'pointermove',
    (e) => {
      const p = pointers.get(e.pointerId)
      if (!p) return
      if (pointers.size === 2) {
        const other = [...pointers.entries()].find(([id]) => id !== e.pointerId)![1]
        const before = Math.hypot(p.x - other.x, p.y - other.y),
          after = Math.hypot(e.clientX - other.x, e.clientY - other.y)
        renderer.zoom((after - before) * 3)
      } else if (pointers.size === 1) {
        if (Math.hypot(e.clientX - p.startX, e.clientY - p.startY) > 5) p.moved = true
        if (p.moved) renderer.rotate(e.clientX - p.x, e.clientY - p.y)
      }
      p.x = e.clientX
      p.y = e.clientY
      onMove()
    },
    { signal },
  )
  const finish = (e: PointerEvent) => {
    const p = pointers.get(e.pointerId)
    if (p && !p.moved && e.type === 'pointerup') {
      const hit = renderer.pick(e.clientX, e.clientY)
      if (hit) onPick(hit)
    }
    pointers.delete(e.pointerId)
    if (canvas.hasPointerCapture(e.pointerId)) canvas.releasePointerCapture(e.pointerId)
  }
  canvas.addEventListener('pointerup', finish, { signal })
  canvas.addEventListener('pointercancel', finish, { signal })
  canvas.addEventListener(
    'wheel',
    (e) => {
      if (e.ctrlKey || e.metaKey) return
      e.preventDefault()
      renderer.zoom(-e.deltaY)
    },
    { passive: false, signal },
  )
  canvas.addEventListener(
    'keydown',
    (e) => {
      if (e.ctrlKey || e.metaKey) return
      if (e.key === '+' || e.key === '=') renderer.zoom(150)
      else if (e.key === '-') renderer.zoom(-150)
      else if (e.key === 'Home') renderer.resetView()
      else if (e.key === 'Enter') {
        const r = canvas.getBoundingClientRect(),
          p = renderer.pick(r.left + r.width / 2, r.top + r.height / 2)
        if (p) onPick(p)
      } else if (e.key.startsWith('Arrow')) {
        e.preventDefault()
        renderer.rotate(
          e.key === 'ArrowLeft' ? -20 : e.key === 'ArrowRight' ? 20 : 0,
          e.key === 'ArrowUp' ? -20 : e.key === 'ArrowDown' ? 20 : 0,
        )
      }
      onMove()
    },
    { signal },
  )
  return controller
}
