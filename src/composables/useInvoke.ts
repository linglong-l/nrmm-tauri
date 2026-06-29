// 调用封装组合式函数模块。
// 该模块提供一个泛型的异步调用封装，用于在组件中以统一方式管理异步操作的
// loading / error / data 三态，避免在每个调用处重复编写 try/catch 与状态切换样板代码。
import { ref } from 'vue';

/**
 * 异步调用封装组合式函数。
 *
 * 作用：
 * - 包装一个返回 Promise 的函数，提供 loading、error、data 三个响应式状态；
 * - 在执行前自动重置 error、置 loading 为 true；
 * - 执行成功后将结果写入 data 并返回；
 * - 执行失败时将错误信息写入 error 并重新抛出，便于调用方进一步处理；
 * - 无论成功/失败，最终都会将 loading 置为 false。
 *
 * 业务逻辑：
 * - 适用于 Tauri invoke、网络请求、本地异步计算等任意返回 Promise 的场景；
 * - 调用方可在模板中直接绑定 loading/error/data，无需额外声明 ref；
 * - `execute` 可被多次调用，每次都会覆盖上次的结果与错误状态。
 *
 * 限制条件：
 * - `fn` 应为无参函数，所需参数请通过闭包提前绑定；
 * - 失败时错误会被重新抛出，调用方需自行 try/catch 以避免未处理 Promise 拒绝；
 * - 不内置重试与去重逻辑，由调用方按需实现。
 *
 * @typeParam T 异步函数返回值的类型
 * @param fn 待包装的异步函数
 * @returns 包含 loading/error/data 响应式状态与 execute 方法的对象
 */
export function useInvoke<T>(fn: () => Promise<T>) {
  // 是否正在执行异步操作
  const loading = ref(false);
  // 错误信息（成功时为 null）
  const error = ref<string | null>(null);
  // 最近一次成功的结果（未执行或失败时为 null）
  const data = ref<T | null>(null);

  /**
   * 执行被包装的异步函数。
   * - 执行前置 loading 为 true、清空 error；
   * - 成功后将结果写入 data 并返回；
   * - 失败时将错误信息字符串写入 error 并重新抛出；
   * - finally 中将 loading 置为 false。
   * @returns 异步函数的返回结果
   */
  const execute = async () => {
    loading.value = true;
    error.value = null;
    try {
      const result = await fn();
      data.value = result;
      return result;
    } catch (e) {
      // 统一将异常转为字符串消息，便于在 UI 上展示
      error.value = e instanceof Error ? e.message : String(e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  return {
    loading,
    error,
    data,
    execute,
  };
}
