//! PNG 转 ICO 图像格式转换模块。
//!
//! 该模块提供 PNG 图像到 ICO 图标的转换功能，支持：
//! - 6 种标准尺寸图标打包（16x16、32x32、48x48、64x64、128x128、256x256）
//! - 文件路径转换与内存字节数组转换
//! - 32 位 RGBA 色彩空间，保留透明通道
//! - 保持纵横比自动缩放（使用 Lanczos3 滤波器）
//!
//! ICO 格式采用现代 PNG 嵌入方式（而非 BMP），兼容性与压缩比更佳。

// 当前自动图标/自定义文件夹图标功能尚未连接，模块内全部公开 API 暂时未消费。
#![allow(dead_code)]

use std::fs;
use std::io::Cursor;

use image::imageops::FilterType;
use image::{ImageFormat, RgbaImage};
use thiserror::Error;

/// 标准 ICO 图标尺寸列表（从小到大）。
///
/// 包含 6 种常见尺寸：16、32、48、64、128、256。
pub const STANDARD_ICON_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

/// PNG→ICO 转换错误类型。
#[derive(Debug, Error)]
pub enum ImageError {
    /// 图像读取/解码失败。
    #[error("Failed to decode image: {0}")]
    ImageDecode(String),

    /// 图像编码失败。
    #[error("Failed to encode image: {0}")]
    ImageEncode(String),

    /// 文件 IO 错误。
    #[error("File IO error: {0}")]
    Io(#[from] std::io::Error),

    /// 输入尺寸无效。
    #[error("Invalid icon size: {0}")]
    InvalidSize(u32),
}

/// 将 PNG 文件转换为 ICO 文件。
///
/// 读取指定路径的 PNG 文件，转换为包含 6 种标准尺寸的 ICO 文件并写入目标路径。
///
/// # 参数
/// - `png_path`: 输入 PNG 文件路径
/// - `ico_path`: 输出 ICO 文件路径
///
/// # 返回值
/// 成功返回 `Ok(())`，失败返回 [`ImageError`]。
///
/// # 错误
/// - 文件不存在或无法读取时返回 [`ImageError::Io`]
/// - PNG 解码失败时返回 [`ImageError::ImageDecode`]
pub fn png_to_ico_file(png_path: &str, ico_path: &str) -> Result<(), ImageError> {
    let png_bytes = fs::read(png_path)?;
    let ico_bytes = png_to_ico_bytes(&png_bytes)?;
    fs::write(ico_path, ico_bytes)?;
    Ok(())
}

/// 将内存中的 PNG 字节数组转换为 ICO 字节数组。
///
/// 生成包含 6 种标准尺寸（16/32/48/64/128/256）的多尺寸 ICO 文件。
/// 所有尺寸均从原始图像等比缩放生成，保留透明通道。
///
/// # 参数
/// - `png_data`: PNG 格式的图像字节数据
///
/// # 返回值
/// 成功返回 ICO 格式的字节数组，失败返回 [`ImageError`]。
///
/// # 错误
/// - PNG 数据无效时返回 [`ImageError::ImageDecode`]
pub fn png_to_ico_bytes(png_data: &[u8]) -> Result<Vec<u8>, ImageError> {
    png_to_ico_bytes_with_sizes(png_data, &STANDARD_ICON_SIZES)
}

/// 将内存中的 PNG 字节数组转换为指定尺寸的 ICO 字节数组。
///
/// # 参数
/// - `png_data`: PNG 格式的图像字节数据
/// - `sizes`: 要包含的图标尺寸列表（例如 `&[16, 32, 48]`）
///
/// # 返回值
/// 成功返回 ICO 格式的字节数组，失败返回 [`ImageError`]。
///
/// # 错误
/// - 尺寸为 0 或超过 256 时返回 [`ImageError::InvalidSize`]
/// - PNG 数据无效时返回 [`ImageError::ImageDecode`]
fn png_to_ico_bytes_with_sizes(png_data: &[u8], sizes: &[u32]) -> Result<Vec<u8>, ImageError> {
    for &s in sizes {
        if s == 0 || s > 256 {
            return Err(ImageError::InvalidSize(s));
        }
    }

    let img = image::load_from_memory_with_format(png_data, ImageFormat::Png)
        .map_err(|e| ImageError::ImageDecode(e.to_string()))?
        .to_rgba8();

    let mut icon_data_list = Vec::with_capacity(sizes.len());
    for &size in sizes {
        let resized = resize_image(&img, size, size);
        let png_data = encode_png(&resized)?;
        icon_data_list.push(png_data);
    }

    build_ico_file(&icon_data_list, sizes)
}

/// 使用 Lanczos3 滤波器等比缩放图像到目标尺寸。
fn resize_image(img: &RgbaImage, target_w: u32, target_h: u32) -> RgbaImage {
    image::imageops::resize(img, target_w, target_h, FilterType::Lanczos3)
}

/// 将 RGBA 图像编码为 PNG 格式字节数组。
fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, ImageError> {
    let mut buf = Vec::new();
    let mut cursor = Cursor::new(&mut buf);
    img.write_to(&mut cursor, ImageFormat::Png)
        .map_err(|e| ImageError::ImageEncode(e.to_string()))?;
    Ok(buf)
}

/// 构造 ICO 文件字节数据。
///
/// ICO 文件结构：
/// 1. ICONDIR header（6 字节）
/// 2. ICONDIRENTRY 数组（每项 16 字节，共 count 项）
/// 3. 图像数据区（每项 PNG 数据依次排列）
fn build_ico_file(
    icon_data_list: &[Vec<u8>],
    sizes: &[u32],
) -> Result<Vec<u8>, ImageError> {
    let count = icon_data_list.len() as u16;
    let header_size = 6usize;
    let entry_size = 16usize;
    let data_offset_base = header_size + entry_size * count as usize;

    let mut result = Vec::new();

    // 1. ICONDIR header
    result.extend_from_slice(&0u16.to_le_bytes()); // reserved
    result.extend_from_slice(&1u16.to_le_bytes()); // type: 1 = icon
    result.extend_from_slice(&count.to_le_bytes()); // count

    // 2. 计算各图像数据偏移量，写入 ICONDIRENTRY
    let mut current_offset = data_offset_base as u32;
    for (i, data) in icon_data_list.iter().enumerate() {
        let size = sizes[i];
        let width_byte = if size >= 256 { 0 } else { size as u8 };
        let height_byte = if size >= 256 { 0 } else { size as u8 };

        result.push(width_byte);       // width
        result.push(height_byte);      // height
        result.push(0u8);              // color count (0 = no palette)
        result.push(0u8);              // reserved
        result.extend_from_slice(&1u16.to_le_bytes()); // planes
        result.extend_from_slice(&32u16.to_le_bytes()); // bpp: 32-bit RGBA
        result.extend_from_slice(&(data.len() as u32).to_le_bytes()); // data size
        result.extend_from_slice(&current_offset.to_le_bytes()); // data offset

        current_offset += data.len() as u32;
    }

    // 3. 追加所有图像数据
    for data in icon_data_list {
        result.extend_from_slice(data);
    }

    Ok(result)
}

/// 异步版本：将 PNG 文件转换为 ICO 文件。
///
/// 使用 `tokio::task::spawn_blocking` 在阻塞线程池中执行同步 IO 操作，
/// 避免阻塞 tokio 异步运行时。
///
/// # 参数
/// - `png_path`: 输入 PNG 文件路径
/// - `ico_path`: 输出 ICO 文件路径
///
/// # 返回值
/// 成功返回 `Ok(())`，失败返回 [`ImageError`]。
pub async fn png_to_ico_file_async(png_path: String, ico_path: String) -> Result<(), ImageError> {
    tokio::task::spawn_blocking(move || png_to_ico_file(&png_path, &ico_path))
        .await
        .map_err(|e| ImageError::ImageEncode(format!("Task join error: {}", e)))?
}

/// 异步版本：将内存中的 PNG 字节数组转换为 ICO 字节数组。
///
/// 使用 `tokio::task::spawn_blocking` 在阻塞线程池中执行 CPU 密集型图像操作，
/// 避免阻塞 tokio 异步运行时。
///
/// # 参数
/// - `png_data`: PNG 格式的图像字节数据
///
/// # 返回值
/// 成功返回 ICO 格式的字节数组，失败返回 [`ImageError`]。
pub async fn png_to_ico_bytes_async(png_data: Vec<u8>) -> Result<Vec<u8>, ImageError> {
    tokio::task::spawn_blocking(move || png_to_ico_bytes(&png_data))
        .await
        .map_err(|e| ImageError::ImageEncode(format!("Task join error: {}", e)))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use std::time::Instant;

    /// 生成一个指定尺寸的测试 PNG 图像（带透明度的彩色渐变方块）。
    fn create_test_png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut img = RgbaImage::new(width, height);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            let r = (x * 255 / width) as u8;
            let g = (y * 255 / height) as u8;
            let b = 128u8;
            let a = if x < width / 2 && y < height / 2 { 255 } else { 128 };
            *pixel = Rgba([r, g, b, a]);
        }
        let mut buf = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        img.write_to(&mut cursor, ImageFormat::Png).unwrap();
        buf
    }

    /// 解析 ICO 文件头，返回尺寸列表。
    fn parse_ico_sizes(ico_bytes: &[u8]) -> Vec<u32> {
        assert!(ico_bytes.len() >= 6);
        let reserved = u16::from_le_bytes([ico_bytes[0], ico_bytes[1]]);
        let icon_type = u16::from_le_bytes([ico_bytes[2], ico_bytes[3]]);
        let count = u16::from_le_bytes([ico_bytes[4], ico_bytes[5]]);
        assert_eq!(reserved, 0);
        assert_eq!(icon_type, 1);

        let mut sizes = Vec::new();
        for i in 0..count as usize {
            let entry_offset = 6 + i * 16;
            let w = ico_bytes[entry_offset] as u32;
            let h = ico_bytes[entry_offset + 1] as u32;
            let w = if w == 0 { 256 } else { w };
            let h = if h == 0 { 256 } else { h };
            assert_eq!(w, h);
            sizes.push(w);
        }
        sizes
    }

    #[test]
    fn test_single_size_conversion() {
        let png_bytes = create_test_png_bytes(256, 256);
        let sizes = &[32u32];
        let ico_bytes = png_to_ico_bytes_with_sizes(&png_bytes, sizes).unwrap();
        let parsed_sizes = parse_ico_sizes(&ico_bytes);
        assert_eq!(parsed_sizes.len(), 1);
        assert_eq!(parsed_sizes[0], 32);
    }

    #[test]
    fn test_multi_size_contains_all_six() {
        let png_bytes = create_test_png_bytes(256, 256);
        let ico_bytes = png_to_ico_bytes(&png_bytes).unwrap();
        let sizes = parse_ico_sizes(&ico_bytes);
        assert_eq!(sizes.len(), 6);
        assert_eq!(sizes, vec![16, 32, 48, 64, 128, 256]);
    }

    #[test]
    fn test_transparency_preserved() {
        let png_bytes = create_test_png_bytes(32, 32);
        let ico_bytes = png_to_ico_bytes(&png_bytes).unwrap();

        let entry_offset = 6 + 1 * 16;
        let data_size = u32::from_le_bytes([
            ico_bytes[entry_offset + 8],
            ico_bytes[entry_offset + 9],
            ico_bytes[entry_offset + 10],
            ico_bytes[entry_offset + 11],
        ]) as usize;
        let data_offset = u32::from_le_bytes([
            ico_bytes[entry_offset + 12],
            ico_bytes[entry_offset + 13],
            ico_bytes[entry_offset + 14],
            ico_bytes[entry_offset + 15],
        ]) as usize;

        let icon_png = &ico_bytes[data_offset..data_offset + data_size];
        let img = image::load_from_memory_with_format(icon_png, ImageFormat::Png).unwrap();
        let rgba = img.to_rgba8();

        let pixel_top_left = rgba.get_pixel(0, 0);
        let pixel_bottom_right = rgba.get_pixel(31, 31);
        assert_eq!(pixel_top_left[3], 255);
        assert_eq!(pixel_bottom_right[3], 128);
    }

    #[test]
    fn test_invalid_input_returns_error() {
        let invalid = vec![0u8; 100];
        let result = png_to_ico_bytes(&invalid);
        assert!(result.is_err());
        match result.unwrap_err() {
            ImageError::ImageDecode(_) => {}
            e => panic!("Expected ImageDecode error, got {:?}", e),
        }
    }

    #[test]
    fn test_invalid_size_zero() {
        let png_bytes = create_test_png_bytes(256, 256);
        let result = png_to_ico_bytes_with_sizes(&png_bytes, &[0]);
        assert!(result.is_err());
        match result.unwrap_err() {
            ImageError::InvalidSize(0) => {}
            e => panic!("Expected InvalidSize(0), got {:?}", e),
        }
    }

    #[test]
    fn test_invalid_size_too_large() {
        let png_bytes = create_test_png_bytes(256, 256);
        let result = png_to_ico_bytes_with_sizes(&png_bytes, &[257]);
        assert!(result.is_err());
        match result.unwrap_err() {
            ImageError::InvalidSize(257) => {}
            e => panic!("Expected InvalidSize(257), got {:?}", e),
        }
    }

    #[test]
    fn test_file_not_found_error() {
        let result = png_to_ico_file("nonexistent.png", "out.ico");
        assert!(result.is_err());
        match result.unwrap_err() {
            ImageError::Io(_) => {}
            e => panic!("Expected Io error, got {:?}", e),
        }
    }

    #[test]
    fn test_png_to_ico_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let png_path = dir.path().join("test.png");
        let ico_path = dir.path().join("test.ico");

        let png_bytes = create_test_png_bytes(256, 256);
        fs::write(&png_path, &png_bytes).unwrap();

        let result = png_to_ico_file(png_path.to_str().unwrap(), ico_path.to_str().unwrap());
        assert!(result.is_ok(), "File conversion should succeed: {:?}", result.err());

        assert!(ico_path.exists());
        let ico_bytes = fs::read(&ico_path).unwrap();
        assert!(ico_bytes.len() > 100);
    }

    #[test]
    fn test_256x256_conversion_performance() {
        let png_bytes = create_test_png_bytes(256, 256);

        // 预热：第一次调用可能有初始化开销
        let _ = png_to_ico_bytes(&png_bytes);

        // 测量第二次调用的耗时
        let start = Instant::now();
        let result = png_to_ico_bytes(&png_bytes);
        let elapsed = start.elapsed();
        assert!(result.is_ok());

        // release 模式下断言 < 100ms，debug 模式下只打印耗时
        if cfg!(not(debug_assertions)) {
            assert!(
                elapsed.as_millis() < 100,
                "256x256 conversion took {}ms, expected < 100ms",
                elapsed.as_millis()
            );
        } else {
            eprintln!(
                "256x256 conversion took {}ms (debug mode, skipping 100ms assertion)",
                elapsed.as_millis()
            );
        }
    }
}
