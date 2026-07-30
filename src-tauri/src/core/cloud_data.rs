//! 云端数据管理模块
//!
//! 负责从远程仓库（Gitee + GitHub 双源）拉取和缓存云端数据：
//! - links.json: 有用链接列表
//! - messages.json: 公告消息
//! - auto_icons.json: 自动图标映射库
//! - known_libraries.json: 已知 3Dmigoto 库列表
//!
//! 数据加载优先级：本地缓存 > 内置资源 > 编译时 include_bytes
//! 启动时后台自动刷新所有云端数据。

use anyhow::Result;
use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};
use crate::config::app_paths;

/// Gitee 仓库 raw 文件基础 URL（国内优先）
static GITEE_RAW_BASE: &str = "https://gitee.com/Yezi26/nrmm-tauri/raw/master";
/// GitHub 仓库 raw 文件基础 URL（海外备用）
static GITHUB_RAW_BASE: &str = "https://raw.githubusercontent.com/linglong-l/nrmm-tauri/master";

/// 需要同步的云端数据文件列表
pub static CLOUD_DATA_FILES: &[&str] = &[
    "links.json",
    "messages.json",
    "auto_icons.json",
    "known_libraries.json",
];

/// 云端数据文件信息结构体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudDataInfo {
    /// 文件名
    pub file_name: String,
    /// Gitee 下载 URL
    pub gitee_url: String,
    /// GitHub 下载 URL
    pub github_url: String,
    /// 本地缓存路径
    pub local_path: PathBuf,
    /// 最后更新时间戳
    pub last_updated: Option<i64>,
}

/// 云端数据管理器（空结构体，仅提供关联方法）
pub struct CloudDataManager;

impl CloudDataManager {
    /// 获取云端数据缓存目录路径（自动创建）
    fn cache_dir() -> Result<PathBuf> {
        let dir = app_paths::cloud_cache_dir();
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// 获取指定文件的缓存路径
    fn cache_path(file_name: &str) -> Result<PathBuf> {
        Ok(Self::cache_dir()?.join(file_name))
    }

    /// 查找内置资源文件路径（打包在应用程序中的 data 文件）
    ///
    /// 搜索位置：
    /// 1. exe 目录/resources/data/
    /// 2. exe 目录/resources/
    /// 3. exe 目录/data/
    /// 4. exe 目录/
    fn bundled_data_path(file_name: &str) -> Option<PathBuf> {
        if let Ok(exe_dir) = std::env::current_exe() {
            let exe_parent = exe_dir.parent()?;
            let candidates = [
                exe_parent.join("resources").join("data").join(file_name),
                exe_parent.join("resources").join(file_name),
                exe_parent.join("data").join(file_name),
                exe_parent.join(file_name),
            ];
            for c in &candidates {
                if c.exists() {
                    return Some(c.clone());
                }
            }
        }
        None
    }

    /// 加载云端数据（反序列化为指定类型）
    ///
    /// 加载优先级：
    /// 1. 本地缓存文件（cache_dir）
    /// 2. 内置资源文件（bundled）
    /// 3. 编译时 include_bytes 嵌入的默认数据（仅已知文件）
    ///
    /// # 类型参数
    /// - `T`: 反序列化目标类型，需要实现 DeserializeOwned
    pub fn load_data<T: serde::de::DeserializeOwned>(file_name: &str) -> Result<T> {
        let cache_path = Self::cache_path(file_name)?;

        // 1. 优先使用本地缓存
        if cache_path.exists() {
            let content = fs::read_to_string(&cache_path)?;
            if let Ok(data) = serde_json::from_str::<T>(&content) {
                return Ok(data);
            }
        }

        // 2. 尝试加载内置资源文件
        if let Some(bundled) = Self::bundled_data_path(file_name) {
            let content = fs::read_to_string(&bundled)?;
            if let Ok(data) = serde_json::from_str::<T>(&content) {
                // 加载成功后写入缓存
                let _ = fs::write(&cache_path, &content);
                return Ok(data);
            }
        }

        // 3. 最后 fallback 到编译时嵌入的数据
        match file_name {
            "links.json" => Ok(serde_json::from_slice(crate::resources::LINKS_JSON)?),
            "messages.json" => Ok(serde_json::from_slice(crate::resources::MESSAGES_JSON)?),
            "auto_icons.json" => Ok(serde_json::from_slice(crate::resources::AUTO_ICONS_JSON)?),
            "known_libraries.json" => Ok(serde_json::from_slice(crate::resources::KNOWN_LIBRARIES_JSON)?),
            _ => anyhow::bail!("Unknown data file: {}", file_name),
        }
    }

    /// 异步刷新单个云端数据文件
    ///
    /// 网络请求优先 Gitee，失败后 fallback 到 GitHub。
    /// 下载成功后验证 JSON 有效性，再写入本地缓存。
    ///
    /// # 参数
    /// - `file_name`: 要刷新的文件名
    pub async fn refresh_async(file_name: &str) -> Result<()> {
        let gitee_url = format!("{}/src-tauri/src/resources/data/{}", GITEE_RAW_BASE, file_name);
        let github_url = format!("{}/src-tauri/src/resources/data/{}", GITHUB_RAW_BASE, file_name);
        let file_name_owned = file_name.to_string();

        let client = reqwest::Client::new();

        // 先尝试 Gitee，失败后尝试 GitHub
        let text = match client.get(&gitee_url).send().await {
            Ok(resp) if resp.status().is_success() => resp.text().await?,
            _ => {
                let resp = client.get(&github_url).send().await?;
                if !resp.status().is_success() {
                    anyhow::bail!("Both Gitee and GitHub fetch failed for {}", file_name);
                }
                resp.text().await?
            }
        };

        // 验证下载内容是有效 JSON
        let _ = serde_json::from_str::<serde_json::Value>(&text)?;

        // 写缓存文件是阻塞 IO，移入 spawn_blocking
        let text_clone = text.clone();
        tauri::async_runtime::spawn_blocking(move || -> Result<()> {
            let cache_path = Self::cache_path(&file_name_owned)?;
            fs::write(&cache_path, text_clone)?;
            Ok(())
        })
        .await??;

        log::info!("Cloud data refreshed: {}", file_name);
        Ok(())
    }

    /// 异步刷新所有云端数据文件
    ///
    /// 返回每个文件的刷新结果（文件名 + Result）
    pub async fn refresh_all_async() -> Vec<(String, Result<()>)> {
        let mut results = Vec::new();
        for file in CLOUD_DATA_FILES {
            let result = Self::refresh_async(file).await;
            results.push((file.to_string(), result));
        }
        results
    }
}

/// Tauri 命令：刷新单个云端数据文件
#[tauri::command]
pub async fn refresh_cloud_data(file_name: String) -> Result<(), String> {
    CloudDataManager::refresh_async(&file_name).await.map_err(|e| e.to_string())
}

/// Tauri 命令：刷新所有云端数据文件
///
/// 返回错误列表（空列表表示全部成功）
#[tauri::command]
pub async fn refresh_all_cloud_data() -> Result<Vec<String>, String> {
    let results = CloudDataManager::refresh_all_async().await;
    let mut errors = Vec::new();
    for (name, res) in results {
        if let Err(e) = res {
            errors.push(format!("{}: {}", name, e));
        }
    }
    if errors.is_empty() {
        Ok(Vec::new())
    } else {
        Ok(errors)
    }
}
