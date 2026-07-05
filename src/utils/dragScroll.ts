/**
 * 鼠标拖动滚动 Hook
 *
 * @deprecated 该 hook 已被 ModsTab.vue 内建拖动滚动逻辑取代。
 * 新代码请勿使用，避免事件监听器重复绑定或冲突。
 * 保留文件仅用于向后兼容。
 *
 * 提供按住鼠标左键拖动内容进行滚动的功能，支持双向滚动（水平和垂直），
 * 并使用 requestAnimationFrame 确保平滑滚动动画（60fps）。
 *
 * 事件冲突处理机制：
 * - 忽略右键和中键点击
 * - 忽略输入元素（input、textarea、select）上的点击，确保表单控件正常工作
 * - 忽略带有 cursor: pointer 样式的元素，避免影响按钮等交互元素
 *
 * @param containerRef - 滚动容器的 ref
 * @returns - 清理函数，用于手动取消事件监听
 */
import { onMounted, onUnmounted } from 'vue';
import type { Ref } from 'vue';

export function useDragScroll(containerRef: Ref<HTMLElement | null>): () => void {
    /** 是否正在拖动 */
    let isDragging = false;
    /** 拖动开始时的鼠标 X 坐标 */
    let startX = 0;
    /** 拖动开始时的鼠标 Y 坐标 */
    let startY = 0;
    /** 拖动开始时的容器水平滚动位置 */
    let scrollLeft = 0;
    /** 拖动开始时的容器垂直滚动位置 */
    let scrollTop = 0;
    /** requestAnimationFrame 的 ID，用于取消未完成的动画帧 */
    let animationFrameId: number | null = null;
    /** 拖动位移阈值（像素），超过此值才视为拖动，避免误触 */
    const dragThreshold = 5;
    /** 记录初始点击位置，用于判断是否超过拖动阈值 */
    let initialClickX = 0;
    let initialClickY = 0;

    /**
     * 鼠标按下事件处理函数
     * @param e - 鼠标事件
     */
    const handleMouseDown = (e: MouseEvent) => {
        // 忽略右键和中键
        if (e.button !== 0) return;

        // 忽略输入元素上的点击（表单控件需要正常交互）
        if (
            e.target instanceof HTMLInputElement ||
            e.target instanceof HTMLTextAreaElement ||
            e.target instanceof HTMLSelectElement ||
            e.target instanceof HTMLButtonElement ||
            (e.target as HTMLElement).tagName === 'BUTTON' ||
            (e.target as HTMLElement).closest('button') !== null ||
            (e.target as HTMLElement).closest('input') !== null ||
            (e.target as HTMLElement).closest('textarea') !== null ||
            (e.target as HTMLElement).closest('select') !== null
        ) {
            return;
        }

        // 获取元素的计算样式，检查是否为指针样式（避免影响按钮等交互元素）
        const target = e.target as HTMLElement;
        const computedStyle = window.getComputedStyle(target);
        if (computedStyle.cursor === 'pointer') {
            return;
        }

        // 记录初始状态
        isDragging = true;
        initialClickX = e.pageX;
        initialClickY = e.pageY;
        startX = e.pageX - (containerRef.value?.offsetLeft || 0);
        startY = e.pageY - (containerRef.value?.offsetTop || 0);
        scrollLeft = containerRef.value?.scrollLeft || 0;
        scrollTop = containerRef.value?.scrollTop || 0;

        // 添加全局鼠标移动和释放事件监听
        document.addEventListener('mousemove', handleMouseMove);
        document.addEventListener('mouseup', handleMouseUp);
        document.addEventListener('mouseleave', handleMouseUp);
    };

    /**
     * 鼠标移动事件处理函数
     * @param e - 鼠标事件
     */
    const handleMouseMove = (e: MouseEvent) => {
        if (!isDragging || !containerRef.value) return;

        // 计算位移，判断是否超过拖动阈值
        const deltaX = Math.abs(e.pageX - initialClickX);
        const deltaY = Math.abs(e.pageY - initialClickY);

        // 未超过阈值时不执行滚动，避免点击时的微小移动被误判为拖动
        if (deltaX < dragThreshold && deltaY < dragThreshold) {
            return;
        }

        // 阻止默认行为，防止文本选择等干扰
        e.preventDefault();

        // 计算当前鼠标相对于容器的位置
        const x = e.pageX - (containerRef.value.offsetLeft || 0);
        const y = e.pageY - (containerRef.value.offsetTop || 0);

        // 计算需要滚动的距离
        const walkX = (x - startX) * 1;
        const walkY = (y - startY) * 1;

        // 使用 requestAnimationFrame 确保平滑滚动（60fps）
        if (animationFrameId) {
            cancelAnimationFrame(animationFrameId);
        }
        animationFrameId = requestAnimationFrame(() => {
            if (containerRef.value) {
                containerRef.value.scrollLeft = scrollLeft - walkX;
                containerRef.value.scrollTop = scrollTop - walkY;
            }
        });
    };

    /**
     * 鼠标释放事件处理函数
     */
    const handleMouseUp = () => {
        isDragging = false;

        // 取消未完成的动画帧
        if (animationFrameId) {
            cancelAnimationFrame(animationFrameId);
            animationFrameId = null;
        }

        // 移除全局事件监听
        document.removeEventListener('mousemove', handleMouseMove);
        document.removeEventListener('mouseup', handleMouseUp);
        document.removeEventListener('mouseleave', handleMouseUp);
    };

    /**
     * 清理函数：移除所有事件监听
     */
    const cleanup = () => {
        containerRef.value?.removeEventListener('mousedown', handleMouseDown);
        handleMouseUp();
    };

    // 组件挂载时绑定鼠标按下事件
    onMounted(() => {
        containerRef.value?.addEventListener('mousedown', handleMouseDown);
    });

    // 组件卸载时清理事件监听
    onUnmounted(cleanup);

    return cleanup;
}