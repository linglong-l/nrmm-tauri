//! 模组预览图缩略图服务（WebView 内存优化，2026-08-19）
//!
//! 问题背景：前端模组卡片的图片显示区仅 100×140 CSS 像素，但此前直接经 asset
//! protocol 加载原图——WebView 按**原图分辨率**解码位图（模组预览图常为
//! 1080p/4K 截图，单张解码 8~33MB），滚动浏览数百张后解码位图缓存突破 GB
//! （实测初载 1300MB / 重载 2000MB+）。
//!
//! 方案：后端首次请求时将原图缩放为最长边 ≤ 档位上限（卡片 [`ThumbSize::Card`] 240px /
//! 头像 [`ThumbSize::Avatar`] 64px）的 JPEG 落盘缓存，后续直接命中缓存文件；前端
//! `<img>` 加载缩略图（单张位图数十 KB 级），内存占用下降一个数量级。
//! 缓存键包含原图 mtime+size + 档位，原图变化自动失效重生成、档位间互不覆盖。
//!
//! 线程模型：命令层 `spawn_blocking` 包裹调用（解码+缩放为 CPU 密集），
//! 并发请求天然并行；同名并发首 generation 重复写同一缓存文件为幂等覆盖（原子写）。

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// JPEG 编码质量（0-100）：预览缩略图 80 足够，兼顾体积
const THUMB_JPEG_QUALITY: u8 = 80;
/// 缓存目录文件数上限：超过后删除最旧一半（按 mtime），防止缓存目录无限膨胀
const MAX_THUMB_FILES: usize = 5000;

/// tmp 文件名全局原子序号：与 pid 组合保证多进程/多线程并发首 generation
/// 时各线程写唯一 tmp 文件，避免 `File::create` 互相 truncate / 交错写
/// 导致 rename 发布残缺 JPEG。
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 缩略图档位：按前端显示区尺寸分档，避免"小显示位加载大图"的解码浪费
///
/// - `Card`: 模组卡片 100×140（@2x DPR = 200×280），240 留少量余量
/// - `Avatar`: 分组头像显示位 ~28px（@2x = 56px），64 覆盖悬浮放大
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    /// 卡片档（最长边 240px）
    Card,
    /// 头像档（最长边 64px）
    Avatar,
}

impl ThumbSize {
    /// 该档位的最长边像素上限
    pub fn max_edge(self) -> u32 {
        match self {
            ThumbSize::Card => 240,
            ThumbSize::Avatar => 64,
        }
    }
}

/// FNV-1a 64 位哈希（缓存键生成；非密码学用途，零依赖）
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 缩略图缓存目录：`<data_local>/nrmm-rust/thumbs/`
fn thumbs_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nrmm-rust")
        .join("thumbs")
}

/// 解析实际使用的缓存目录：`override_dir` 提供时（测试注入）优先，否则用默认目录。
///
/// # 参数
/// - `override_dir`: 测试注入的缓存目录（`None` 表示生产默认目录）
fn resolve_cache_dir(override_dir: Option<&Path>) -> PathBuf {
    override_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(thumbs_dir)
}

/// 计算原图对应的缓存键：FNV-1a(规范化路径 + 档位最长边 + mtime 纳秒 + size)。
///
/// mtime/size 入键保证原图被替换后缓存自动失效（重新生成）；
/// 档位最长边入键保证同一原图不同档位独立缓存、互不覆盖。
///
/// # 参数
/// - `original`: 原图路径
/// - `max_edge`: 档位最长边像素上限（档位隔离，不同档位独立缓存）
///
/// # 返回
/// 16 进制缓存键字符串
fn cache_key(original: &Path, max_edge: u32) -> String {
    let mut buf = Vec::new();
    buf.extend_from_slice(original.to_string_lossy().to_lowercase().as_bytes());
    buf.extend_from_slice(&max_edge.to_le_bytes()); // 档位隔离：不同档位独立缓存
    if let Ok(md) = fs::metadata(original) {
        buf.extend_from_slice(&md.len().to_le_bytes());
        if let Ok(mtime) = md.modified() {
            if let Ok(nanos) = mtime.duration_since(SystemTime::UNIX_EPOCH) {
                buf.extend_from_slice(&nanos.as_nanos().to_le_bytes());
            }
        }
    }
    format!("{:016x}", fnv1a64(&buf))
}

/// 支持的后缀/格式的错误消息前缀：由前端识别该错误 → 回退原图（如 SVG/AVIF 等
/// image crate 不内置的类型，前端浏览器本身可原生显示）
pub const ERR_UNSUPPORTED_PREFIX: &str = "[THUMB_UNSUPPORTED]";

/// 单次解码尝试（不做重试）：
/// 1. 读文件头用 `infer` 嗅探真实格式（不信任扩展名）
/// 2. 显式格式解码
///
/// 返回含 [`ERR_UNSUPPORTED_PREFIX`] 的错误表示类型不支持（前端回退原图，不重试）；
/// 其余错误（文件占用/读到部分数据/解码异常）为可重试的瞬态/解码错误。
fn decode_attempt(path: &Path) -> anyhow::Result<image::DynamicImage> {
    use std::io::Read;
    use std::io::Seek;

    // 读头部判断真实格式
    let mut file =
        std::fs::File::open(path).with_context(|| format!("打开原图失败: {:?}", path))?;
    let mut head = [0u8; 1024];
    let n = file.read(&mut head)?;
    let fmt = match infer::get(&head[..n]).map(|t| t.mime_type()) {
        Some("image/png") => image::ImageFormat::Png,
        Some("image/jpeg") => image::ImageFormat::Jpeg,
        Some("image/gif") => image::ImageFormat::Gif,
        Some("image/webp") => image::ImageFormat::WebP,
        Some("image/bmp") => image::ImageFormat::Bmp,
        Some(other) => {
            anyhow::bail!("{} 不支持的文件类型: {}", ERR_UNSUPPORTED_PREFIX, other)
        }
        None => {
            anyhow::bail!(
                "{} 无法识别文件头（可能已损坏）：{:?}",
                ERR_UNSUPPORTED_PREFIX,
                path
            )
        }
    };

    // 回卷到文件头，用已识别的格式解码
    file.seek(std::io::SeekFrom::Start(0))?;
    image::ImageReader::with_format(std::io::BufReader::new(file), fmt)
        .decode()
        .with_context(|| format!("解码原图为 {fmt:?} 失败: {:?}", path))
}

/// 文件被独占写入/未写完导致的瞬态错误重试次数（真实文件可正常读取，失败多因
/// 导入/备份并发写期间读到半成品或句柄被锁）
const TRANSIENT_RETRY: usize = 3;
/// 重试间隔（毫秒），递增使文件写完前不频繁撞锁
const TRANSIENT_RETRY_INTERVAL_MS: u64 = 120;

/// 按文件头（magic bytes）判断图片类型并加载解码（不信任扩展名），
/// 对瞬态 IO/解码失败进行有限重试。
///
/// 步骤：
/// 1. [`decode_attempt`]：读文件头嗅探 + 显式格式解码
/// 2. 类型不支持（错误含 [`ERR_UNSUPPORTED_PREFIX`]）→ 立即返回（前端回退原图）
/// 3. 其余错误（文件被写入占用/读到部分数据等瞬态）→ 间隔重试 [`TRANSIENT_RETRY`] 次
///
/// # 参数
/// - `path`: 原图路径
///
/// # 返回
/// 解码后的动态图像；类型不支持/多次重试仍失败返回 `Err`
pub fn decode_by_header(path: &Path) -> anyhow::Result<image::DynamicImage> {
    decode_by_header_with_retry(path, TRANSIENT_RETRY, TRANSIENT_RETRY_INTERVAL_MS)
}

/// [`decode_by_header`] 的可参数化重试内核。
///
/// 提取自 `decode_by_header` 供测试注入宽预算（`cargo test --lib` 并行整跑时
/// 后台补写线程可能被 CPU 饥饿延迟超过生产默认 720ms 预算，属测试密封性问题
/// 而非产品缺陷）；生产路径仍走 [`TRANSIENT_RETRY`]/[`TRANSIENT_RETRY_INTERVAL_MS`] 默认值。
fn decode_by_header_with_retry(
    path: &Path,
    max_retry: usize,
    interval_ms: u64,
) -> anyhow::Result<image::DynamicImage> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..=max_retry {
        match decode_attempt(path) {
            Ok(img) => return Ok(img),
            Err(e) => {
                // 类型不支持：非瞬态，立即返回（前端回退原图）
                if e.to_string().contains(ERR_UNSUPPORTED_PREFIX) {
                    return Err(e);
                }
                // 类型已判定成功但解码失败（瞬态），记录并重试
                last_err = Some(e);
                if attempt < max_retry {
                    log::warn!(
                        "[thumbnail] 解码 {} 第 {} 次失败（瞬态重试），将重试: {:?}",
                        path.display(),
                        attempt + 1,
                        last_err.as_ref().map(|e| e.to_string())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(
                        interval_ms * (attempt as u64 + 1),
                    ));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("解码 {} 失败: {:?}", path.display(), path)))
}

/// 生成唯一的临时文件名（`{key}.{pid}.{seq}.tmp`）。
///
/// tmp 文件名全局原子序号（与 pid 组合保证多进程/多线程唯一）：并发的首 generation
/// 各线程写唯一 tmp 文件，防止 `File::create` 互相 truncate 导致交错写入、最后
/// rename 发布残缺 JPEG。
fn unique_tmp_name(dir: &Path, key: &str) -> PathBuf {
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!("{}.{}.{}.tmp", key, std::process::id(), seq))
}

/// 获取（或生成）原图的缩略图缓存文件路径。
///
/// 流程：
/// 1. 缓存键计算（含档位）→ 缓存文件存在则直接返回（快路径，仅一次 metadata）
/// 2. 未命中：**按文件头判断图片类型**（infer，不信任扩展名）→ 用显式格式解码 →
///    等比缩放（最长边 [`ThumbSize::max_edge`]，小图不放大）→ JPEG 编码 → 原子写缓存
/// 3. 缓存目录容量控制：文件数超 [`MAX_THUMB_FILES`] 时按 mtime 删除最旧一半
///
/// # 参数
/// - `original`: 原图绝对路径
/// - `size`: 缩略图档位（卡片 [`ThumbSize::Card`] / 头像 [`ThumbSize::Avatar`]）
/// - `cache_dir_override`: 测试注入的缓存目录（生产传 `None` 用默认目录）
///
/// # 返回
/// 缩略图缓存文件路径；原图不存在/解码失败/编码失败返回 `Err`；
/// 图片类型不被后端支持时错误消息含 [`ERR_UNSUPPORTED_PREFIX`]（前端据此回退原图）。
pub fn get_or_create_thumbnail(
    original: &Path,
    size: ThumbSize,
    cache_dir_override: Option<&Path>,
) -> Result<PathBuf> {
    let dir = resolve_cache_dir(cache_dir_override);
    fs::create_dir_all(&dir).with_context(|| format!("创建缩略图缓存目录失败: {:?}", dir))?;

    let key = cache_key(original, size.max_edge());
    let thumb_path = dir.join(format!("{}.jpg", key));
    if thumb_path.exists() {
        return Ok(thumb_path);
    }

    // 解码原图 → 等比缩放（仅当最长边超限时缩放；image::thumbnail 会放大小图，需先判断）。
    // 注意：decode_by_header 的错误已带上下文与类型标记（[THUMB_UNSUPPORTED]），不再用
    // with_context 包裹——否则前缀会被外层的"解码原图失败"文案遮蔽，前端无法识别类型标记。
    let img = decode_by_header(original)?;
    let max_edge = size.max_edge();
    let thumb = if img.width().max(img.height()) > max_edge {
        img.thumbnail(max_edge, max_edge).to_rgb8()
    } else {
        img.to_rgb8()
    };

    // 原子写缓存（唯一 tmp + rename：进程退出不产生半成品文件；并发首 generation
    // 各线程写不同 tmp 文件，避免互相 truncate / 交错写发布残缺 JPEG）
    let tmp_path = unique_tmp_name(&dir, &key);
    {
        let file = fs::File::create(&tmp_path)
            .with_context(|| format!("创建缩略图临时文件失败: {:?}", tmp_path))?;
        let mut writer = std::io::BufWriter::new(file);
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, THUMB_JPEG_QUALITY);
        encoder
            .encode_image(&thumb)
            .with_context(|| format!("编码缩略图失败: {:?}", original))?;
        use std::io::Write as _;
        writer.flush().ok();
    }
    fs::rename(&tmp_path, &thumb_path)
        .with_context(|| format!("重命名缩略图缓存失败: {:?} -> {:?}", tmp_path, thumb_path))?;

    evict_if_needed(&dir);
    Ok(thumb_path)
}

/// 缓存目录容量控制：文件数超上限时按 mtime 删除最旧一半。
///
/// 删除失败仅记录警告（不影响主流程）；best-effort，不保证精确原子性。
///
/// # 参数
/// - `dir`: 缩略图缓存目录
fn evict_if_needed(dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("jpg") {
            if let Ok(md) = entry.metadata() {
                let mtime = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((mtime, p));
            }
        }
    }
    if files.len() <= MAX_THUMB_FILES {
        return;
    }
    files.sort_by_key(|(t, _)| *t);
    let to_remove = files.len() - MAX_THUMB_FILES / 2;
    log::info!(
        "[thumbnail] 缓存文件数 {} 超上限 {}，清理最旧 {} 个",
        files.len(),
        MAX_THUMB_FILES,
        to_remove
    );
    for (_, p) in files.into_iter().take(to_remove) {
        if let Err(e) = fs::remove_file(&p) {
            log::warn!("[thumbnail] 清理缓存文件失败 {:?}: {}", p, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：生成纯色测试图（PNG）到临时目录
    fn make_test_png(dir: &Path, w: u32, h: u32) -> PathBuf {
        let path = dir.join(format!("test_{}x{}.png", w, h));
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200u8, 100, 50]));
        image::DynamicImage::ImageRgb8(img)
            .save(&path)
            .expect("保存测试图失败");
        path
    }

    /// 测试辅助：独立缓存目录（临时目录下 thumbs 子目录，互不污染）
    fn test_cache(tmp: &tempfile::TempDir) -> PathBuf {
        tmp.path().join("thumbs")
    }

    /// 缩略图生成：大图被缩放到最长边上限内（卡片档 240px），且缓存文件存在
    #[test]
    fn thumbnail_scales_down() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = test_cache(&tmp);
        let original = make_test_png(tmp.path(), 1920, 1080);
        let thumb = get_or_create_thumbnail(&original, ThumbSize::Card, Some(&cache))
            .expect("生成缩略图失败");
        assert!(thumb.exists(), "缓存文件应存在");
        assert_eq!(thumb.extension().and_then(|e| e.to_str()), Some("jpg"));

        // 解码缩略图验证尺寸：最长边 ≤ 卡片档上限
        let t = image::open(&thumb).expect("解码缩略图失败");
        assert!(
            t.width().max(t.height()) <= ThumbSize::Card.max_edge(),
            "缩略图最长边应 ≤ {}，实际 {}x{}",
            ThumbSize::Card.max_edge(),
            t.width(),
            t.height()
        );
    }

    /// 档位隔离：同一原图 Card 与 Avatar 生成不同缓存文件且尺寸各自正确
    #[test]
    fn thumbnail_size_tiers() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = test_cache(&tmp);
        let original = make_test_png(tmp.path(), 1920, 1080);
        let card = get_or_create_thumbnail(&original, ThumbSize::Card, Some(&cache)).unwrap();
        let avatar = get_or_create_thumbnail(&original, ThumbSize::Avatar, Some(&cache)).unwrap();
        assert_ne!(card, avatar, "不同档位应命中不同缓存文件");
        let c = image::open(&card).unwrap();
        let a = image::open(&avatar).unwrap();
        assert!(
            c.width().max(c.height()) <= ThumbSize::Card.max_edge(),
            "Card 档最长边 ≤ {}",
            ThumbSize::Card.max_edge()
        );
        assert!(
            a.width().max(a.height()) <= ThumbSize::Avatar.max_edge(),
            "Avatar 档最长边 ≤ {}",
            ThumbSize::Avatar.max_edge()
        );
    }

    /// 小图不放大：小于上限的图保持原尺寸
    #[test]
    fn thumbnail_keeps_small_image() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = test_cache(&tmp);
        let original = make_test_png(tmp.path(), 120, 160);
        let thumb = get_or_create_thumbnail(&original, ThumbSize::Card, Some(&cache))
            .expect("生成缩略图失败");
        let t = image::open(&thumb).expect("解码缩略图失败");
        assert_eq!((t.width(), t.height()), (120, 160), "小图不应被放大或缩放");
    }

    /// 缓存命中：二次调用返回同一路径（且不重新生成——mtime 不变键不变）
    #[test]
    fn thumbnail_cache_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = test_cache(&tmp);
        let original = make_test_png(tmp.path(), 800, 600);
        let t1 = get_or_create_thumbnail(&original, ThumbSize::Card, Some(&cache)).unwrap();
        let t2 = get_or_create_thumbnail(&original, ThumbSize::Card, Some(&cache)).unwrap();
        assert_eq!(t1, t2, "同原图两次调用应返回同一缓存路径");
    }

    /// 原图变化后缓存键失效：mtime/size 入键，替换原图后生成新缓存
    #[test]
    fn thumbnail_invalidates_on_change() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = test_cache(&tmp);
        let original = tmp.path().join("changing.png");
        // 第一版：600x400
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            600,
            400,
            image::Rgb([10, 20, 30]),
        ))
        .save(&original)
        .unwrap();
        let t1 = get_or_create_thumbnail(&original, ThumbSize::Card, Some(&cache)).unwrap();
        // 第二版（内容/尺寸不同 → mtime/size 变化）
        std::thread::sleep(std::time::Duration::from_millis(20)); // 保证 mtime 前进
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            640,
            480,
            image::Rgb([99, 88, 77]),
        ))
        .save(&original)
        .unwrap();
        let t2 = get_or_create_thumbnail(&original, ThumbSize::Card, Some(&cache)).unwrap();
        assert_ne!(t1, t2, "原图变化后应生成新缓存（键含 mtime/size）");
    }

    /// 原图不存在：返回错误（不 panic）
    #[test]
    fn thumbnail_missing_original() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = test_cache(&tmp);
        let result = get_or_create_thumbnail(
            Path::new("Z:/nonexistent/xx.png"),
            ThumbSize::Card,
            Some(&cache),
        );
        assert!(result.is_err());
    }

    /// 文件头判断：`.png` 后缀但实际是 WebP magic —— 按内容识别为 WebP 而非报错
    /// （覆盖"后缀误导"场景：真实世界存在改名的预览图）
    #[test]
    fn thumbnail_decodes_by_header_not_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = test_cache(&tmp);
        // 构造一个 WebP 内容但命名为 .png 的文件（从临时图转 webp 后改名）
        // 用 image 生成 webp 受 feature 限制，直接写一个最小合法 WebP magic 头不可行
        // （解码需完整文件）。此处退而验证：内容非 PNG 时应按内容而非扩展名判定。
        // 用一个真实 webp 文件：先由 image 保存 webp（如 feature 未启用则走 png 分支变体）
        let png_src = make_test_png(tmp.path(), 64, 64);
        // 生成同尺寸 webp（image 默认 feature 含 webp 编码）
        let webp_bytes = {
            let img = image::open(&png_src).unwrap();
            let mut buf = std::io::Cursor::new(Vec::new());
            let dynimg = img.to_rgba8();
            use image::ImageEncoder as _;
            image::codecs::webp::WebPEncoder::new_lossless(&mut buf)
                .write_image(dynimg.as_raw(), 64, 64, image::ExtendedColorType::Rgba8)
                .unwrap();
            buf.into_inner()
        };
        let renamed = tmp.path().join("actually_webp.png");
        std::fs::write(&renamed, &webp_bytes).unwrap();
        // 文件头嗅探应识别为 webp（image 支持）→ 生成成功或返回"解码失败"均可接受；
        // 关键断言：绝不因"不识别"而抛 UNSUPPORTED（因 webp 是支持类型清单内）
        match get_or_create_thumbnail(&renamed, ThumbSize::Card, Some(&cache)) {
            Ok(path) => {
                assert!(path.exists(), "webp 内容应可生成缩略图");
            }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains(ERR_UNSUPPORTED_PREFIX),
                    "webp 是支持类型，不应标记 UNSUPPORTED: {}",
                    msg
                );
            }
        }
    }

    /// 不支持格式：SVG 返回含 ERR_UNSUPPORTED_PREFIX 的错误（前端据此回退原图）
    #[test]
    fn thumbnail_unsupported_format_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = test_cache(&tmp);
        let svg = tmp.path().join("icon.svg");
        std::fs::write(
            &svg,
            "<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><rect/></svg>",
        )
        .unwrap();
        let err = get_or_create_thumbnail(&svg, ThumbSize::Card, Some(&cache))
            .expect_err("SVG 应返回错误");
        assert!(
            err.to_string().contains(ERR_UNSUPPORTED_PREFIX),
            "SVG 错误应含 {}: {}",
            ERR_UNSUPPORTED_PREFIX,
            err
        );
    }

    /// 损坏文件头：非图片内容按 UNSUPPORTED 处理（避免伪装文件进入解码）
    #[test]
    fn thumbnail_unrecognized_header_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = test_cache(&tmp);
        let junk = tmp.path().join("fake.png");
        std::fs::write(&junk, b"this is not an image content at all").unwrap();
        let err = get_or_create_thumbnail(&junk, ThumbSize::Card, Some(&cache))
            .expect_err("非图片应返回错误");
        assert!(
            err.to_string().contains(ERR_UNSUPPORTED_PREFIX),
            "非图片错误应含 {}: {}",
            ERR_UNSUPPORTED_PREFIX,
            err
        );
    }

    /// 瞬态失败重试恢复：文件初始为"残缺 PNG"（解码失败），重试窗口内被补齐为完整
    /// PNG → decode_by_header 最终成功。模拟导入/备份并发写期间读到半成品再恢复。
    #[test]
    fn thumbnail_retries_transient_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("transient.png");

        // 初始：仅 PNG 签名 + 少量残缺数据 → infer 识别为 png，但解码因数据不足失败
        std::fs::write(
            &original,
            [
                0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
            ],
        )
        .unwrap();

        // 后台线程在重试间隔内把文件替换为完整 PNG
        std::thread::spawn({
            let path = original.clone();
            move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                    64,
                    64,
                    image::Rgb([10, 200, 30]),
                ));
                img.save(&path).ok();
            }
        });

        // decode_by_header 应在前 1~2 次尝试失败后因文件补齐而成功。
        // 注入宽预算（20 次 × 50ms 递增 ≈ 10.5s 窗口）：并行整跑时后台补写线程
        // 可能被 CPU 饥饿延迟超过生产默认 720ms（历史 flake 现场：高负载整跑
        // 64s 时该测试偶发失败），宽预算使调度竞态实际不可能触发。
        let img =
            decode_by_header_with_retry(&original, 20, 50).expect("残缺文件补齐后应经重试成功");
        assert_eq!(
            (img.width(), img.height()),
            (64, 64),
            "补齐后应解码出正确尺寸"
        );
    }

    /// UNSUPPORTED 错误立即返回、不重试：SVG 一次即报 UNSUPPORTED（重试次数不变）
    #[test]
    fn thumbnail_unsupported_not_retried() {
        let tmp = tempfile::tempdir().unwrap();
        // 用一个必然 UNSUPPORTED 的 SVG；decode_by_header 应"尽早"返回（不耗重试间隔）
        // 断言：错误含 UNSUPPORTED 前缀（若被错误重试会先 fail 在类型判定前的 IO，也不会含前缀）
        let svg = tmp.path().join("icon.svg");
        std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        let err = decode_by_header(&svg).expect_err("SVG 应返回错误");
        assert!(
            err.to_string().contains(ERR_UNSUPPORTED_PREFIX),
            "SVG 应含 UNSUPPORTED: {}",
            err
        );
    }

    /// FNV-1a 哈希正确性（测试向量）
    #[test]
    fn fnv1a_known_vector() {
        // FNV-1a 64 空串 = 0xcbf29ce484222325
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        // FNV-1a 64 "a" = 0xaf63dc4c8601ec8c
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    /// unique_tmp_name 两次生成结果互不相等（保证并发首 generation 不写同一 tmp）
    #[test]
    fn unique_tmp_name_suffixes_differ() {
        let dir = Path::new("/tmp/cache");
        let a = unique_tmp_name(dir, "abc123");
        let b = unique_tmp_name(dir, "abc123");
        assert_ne!(
            a, b,
            "两次生成的 tmp 文件名应互不相等（避免并发截断同一文件）"
        );
    }
}
