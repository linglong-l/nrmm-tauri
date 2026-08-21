//! 缩略图命令（WebView 内存优化，2026-08-19）
//!
//! 前端模组卡片图片显示区仅 100×140px，直接加载原图会导致 WebView 按原图
//! 分辨率解码位图（1080p/4K 预览图单张 8~33MB），滚动浏览数百张后内存破 GB。
//! 本命令返回原图缩放后的缓存文件路径，前端 `<img>` 改加载缩略图。

use crate::core::thumbnail;
use anyhow::Result;
use std::path::PathBuf;

/// 获取模组预览图缩略图的本地缓存路径（不存在则生成）。
///
/// 生成/缓存策略见 [`thumbnail::get_or_create_thumbnail`]：
/// 按档位缩放（Card 240px / Avatar 64px）、JPEG q80、缓存键含原图 mtime+size+档位
/// （原图变化自动失效、档位间互不覆盖）、缓存目录容量上限 5000 文件（超限清理最旧一半）。
///
/// 解码+缩放为 CPU 密集操作，走 `spawn_blocking`（不阻塞异步运行时）；
/// 大量卡片并发请求时由 blocking 线程池天然并行，同名原图重复生成幂等。
///
/// # 参数
/// - `original_path`: 原图绝对路径（asset protocol scope 内的图片格式）
/// - `size`: 档位字符串：`"avatar"` → 头像档（64px），其余/缺失 → 卡片档（240px，向后兼容）
///
/// # 返回
/// 缩略图缓存文件绝对路径（JPEG）；前端需再经 `convertFileSrc` 转换为可加载 URL。
/// 原图不存在/解码失败返回 Err（前端应回退原图或占位符）。
#[tauri::command]
pub async fn get_thumbnail(original_path: String, size: Option<String>) -> Result<String, String> {
    // 默认卡片档（向后兼容无参调用）；"avatar" 显式走头像档
    let thumb_size = match size.as_deref() {
        Some("avatar") => thumbnail::ThumbSize::Avatar,
        _ => thumbnail::ThumbSize::Card,
    };
    let original = PathBuf::from(original_path);
    tauri::async_runtime::spawn_blocking(move || {
        thumbnail::get_or_create_thumbnail(&original, thumb_size, None)
            .map(|p| p.to_string_lossy().to_string())
            .map_err(|e| {
                log::warn!("[get_thumbnail] 生成缩略图失败: {}", e);
                e.to_string()
            })
    })
    .await
    .map_err(|e| format!("spawn_blocking join error: {}", e))?
}
