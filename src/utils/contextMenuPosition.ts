/**
 * 右键菜单 fixed 定位坐标计算（含 CSS zoom 缩放校正）
 *
 * ## 根因（缩放倍率不同导致的右键菜单漂移）
 *
 * App.vue::applyScale() 通过 `document.documentElement.style.zoom`（0.6~2.0）实现
 * 全局界面缩放。CSS `zoom` 是**可继承属性**：Teleport 到 body 的右键菜单元素同样
 * 处于被缩放的坐标空间，其 `position: fixed` 的 `left/top` 会先乘以 zoom 再落到
 * 屏幕上；而鼠标事件的 `clientX/clientY` **不受 zoom 影响**（始终为视口物理 CSS 像素）。
 *
 * 因此直接写 `left: clientX` 时，菜单实际落在 `clientX × zoom` 处：
 * zoom=1 时恰好正确，zoom=1.5 时向右/下漂移 50%，zoom=0.6 时向左/上偏 ——
 * 表现为「不同缩放倍率下右键菜单位置不一样」。
 *
 * 修法：坐标统一除以根 zoom，使「菜单本地坐标 × zoom == 光标物理坐标」；
 * 同时视口边界也须换算到本地空间（innerWidth / zoom）后再做溢出收敛。
 */

/**
 * 读取文档根元素上生效的 CSS zoom 值
 *
 * 优先取 App.vue 内联设置的 `documentElement.style.zoom`，回退到 computed style
 * （浏览器不支持 CSS zoom 时为空串）。无法解析或非法（<=0 / NaN）时按 1 处理，
 * 保证「未启用缩放」场景行为与旧实现完全一致。
 */
export function getDocumentZoom(): number {
  let raw = document.documentElement.style.zoom
  if (!raw) {
    raw = getComputedStyle(document.documentElement).getPropertyValue('zoom')
  }
  const z = parseFloat(raw)
  return Number.isFinite(z) && z > 0 ? z : 1
}

/**
 * 计算右键菜单的 fixed 定位坐标（缩放校正 + 视口内收敛）
 *
 * # 参数
 * - `e`: 触发右键的鼠标事件
 * - `menuW` / `menuH`: 菜单估算尺寸（本地坐标空间，即 CSS 声明值）
 * - `margin`: 视口安全边距（本地坐标空间）
 *
 * # 返回
 * 可直接绑定到 `:style="{ top: y + 'px', left: x + 'px' }"` 的坐标
 * （`position: fixed` 元素，坐标已按根 zoom 校正并收敛进视口）
 */
export function placeContextMenu(
  e: MouseEvent,
  menuW: number,
  menuH: number,
  margin = 4,
): { x: number; y: number } {
  const z = getDocumentZoom()
  // 视口边界换算到菜单（被 zoom 缩放）的本地坐标空间
  const vw = window.innerWidth / z
  const vh = window.innerHeight / z
  // clientX/clientY 是物理 CSS 像素 → 除以 zoom 得到本地坐标
  let x = e.clientX / z
  let y = e.clientY / z
  // 溢出收敛：右/下边缘超出则整体左/上移一个菜单位（保证菜单始终完整可见）
  if (x + menuW > vw - margin) {
    x = vw - menuW - margin
  }
  if (y + menuH > vh - margin) {
    y = vh - menuH - margin
  }
  // 菜单尺寸超过视口（极端小窗）时不得出负值
  if (x < margin) x = margin
  if (y < margin) y = margin
  return { x, y }
}
