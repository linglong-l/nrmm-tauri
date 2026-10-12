/**
 * 非法 UTF-8 文件名解码工具（后端 fs_walk base64 保真字节 → 前端展示字符串）
 *
 * 后端对非 UTF-8 文件名做平台编码字节的 base64 传输：
 * - `u16le:<base64>`（Windows）：UTF-16LE 字节，可无损还原孤代理项（JS String 支持）
 * - `raw:<base64>`（Linux/macOS）：OsStr 原始字节，按字节映射展示
 *
 * 展示语义：文件名本身「无法以标准字符显示」，故统一渲染为可读的占位串以便用户定位；
 * 若字节恰好可解码为合法文本（如早期双字节编码），则尽力展示。
 */

/** base64 解码（浏览器 API，兼容 WebView2 / Chromium） */
function base64ToBytes(b64: string): Uint8Array | null {
  try {
    const binary = atob(b64)
    const bytes = new Uint8Array(binary.length)
    for (let i = 0; i < binary.length; i++) {
      bytes[i] = binary.charCodeAt(i)
    }
    return bytes
  } catch {
    return null
  }
}

/**
 * 将后端传输的保真字节串转换为展示字符串
 *
 * 解码失败/未知前缀时返回占位符 `<不可识别>`（带原始 base64 摘要便于排查）。
 *
 * @param nameBytes 后端 `SkippedEntry.nameBytes`（`u16le:`/`raw:` 前缀 + base64）
 * @returns 可展示的文件名（无法识别时为占位符）
 */
function decodeInvalidUtf8Name(nameBytes: string): string {
  const sep = nameBytes.indexOf(':')
  if (sep <= 0) return `<不可识别>`
  const prefix = nameBytes.slice(0, sep)
  const b64 = nameBytes.slice(sep + 1)
  const bytes = base64ToBytes(b64)
  if (!bytes || bytes.length === 0) return `<不可识别>`

  if (prefix === 'u16le') {
    // UTF-16LE：每 2 字节一个 u16，String.fromCharCode 对孤代理项保真
    const units = new Uint16Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 2)
    let out = ''
    for (const u of units) out += String.fromCharCode(u)
    return out
  }

  if (prefix === 'raw') {
    // 原始字节：逐字节映射到 U+0000-U+00FF（latin1，1:1 保真不丢信息）
    let out = ''
    for (const b of bytes) out += String.fromCharCode(b)
    return out
  }

  return `<不可识别>`
}

/**
 * 将非法条目列表格式化为单体提示消息
 *
 * @param count 跳过总数
 * @param entries 明细列表
 * @param parentDisplayFn 可选：父路径展示回调（默认直接使用父路径）
 * @returns 提示消息字符串
 */
export function formatInvalidUtf8Message(
  count: number,
  entries: readonly { parentDisplay: string; nameBytes: string }[],
): string {
  if (count <= 0) return ''
  const head = entries.slice(0, 3)
  const lines = head.map(
    (e) => `${e.parentDisplay}\\<${decodeInvalidUtf8Name(e.nameBytes)}>`,
  )
  const more = count > head.length ? ` 等共 ${count} 个` : ''
  return `发现 ${count} 个因文件名编码无法识别的文件/目录${more}：\n${lines.join('\n')}`
}