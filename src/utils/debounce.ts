/**
 * 创建防抖函数。
 *
 * 在指定延迟内重复调用会重置计时器，仅最后一次调用会执行。
 * 适用于输入框实时搜索、窗口调整事件等高频触发场景。
 *
 * @param fn 需要防抖的异步函数
 * @param delay 延迟毫秒
 * @returns 包装后的防抖函数
 *
 * @example
 * const debouncedSave = createDebounce(async (value: string) => {
 *   await saveToBackend(value);
 * }, 500);
 *
 * input.addEventListener('input', (e) => debouncedSave(e.target.value));
 */
export function createDebounce<T extends (...args: any[]) => Promise<void>>(
  fn: T,
  delay: number
): (...args: Parameters<T>) => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return (...args: Parameters<T>) => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      fn(...args);
    }, delay);
  };
}
