/// <reference types="vite/client" />

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<{}, {}, any>;
  export default component;
}

/**
 * 扩展 Window 接口，声明 File System Access API。
 * 该 API 允许通过 `window.showDirectoryPicker()` 弹出系统目录选择对话框，
 * 返回一个带有 `path` 属性的目录句柄对象。
 */
interface Window {
  showDirectoryPicker?: (options?: DisplayNameOrDirOpts) => Promise<FileSystemDirectoryHandle & { path: string }>;
}

/**
 * 扩展 File 接口，补充 Tauri/WebKit 环境下附加的 `path` 属性。
 * 在通过文件选择或拖放获取 File 对象时，部分运行时会附加文件系统绝对路径。
 */
interface File {
  path?: string;
}
