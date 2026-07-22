/**
 * 将毫秒数格式化为 "X.XX 秒" 字符串。
 *
 * @param durationMs 耗时（毫秒）
 * @returns 格式化后的字符串，精确到小数点后两位
 *
 * @example
 * formatDuration(1523) // "1.52 秒"
 * formatDuration(0)    // "0.00 秒"
 */
export function formatDuration(durationMs: number): string {
  const seconds = durationMs / 1000;
  return `${seconds.toFixed(2)} 秒`;
}
