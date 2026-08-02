//! 应用自更新模块
//!
//! 检查 GitHub/Gitee 双源的最新 Release，支持版本比较。
//! 更新检查策略：优先 Gitee（国内访问快），失败则回退到 GitHub。
//!
//! # 功能
//! - check_for_updates: 异步检查更新，返回最新版本信息
//! - compare_versions: 版本号比较（语义化版本）
//! - get_app_version: 获取当前应用版本

use anyhow::Result;
use serde::{Serialize, Deserialize};

/// Gitee Releases API 地址
static GITEE_RELEASES_API: &str = "https://gitee.com/api/v5/repos/Yezi26/nrmm-tauri/releases/latest";
/// GitHub Releases API 地址
static GITHUB_RELEASES_API: &str = "https://api.github.com/repos/linglong-l/nrmm-tauri/releases/latest";

/// 更新信息结构体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub download_url: String,
    pub release_notes: String,
    pub published_at: String,
    pub source: String,
}

/// 更新管理器
pub struct UpdateManager;

impl UpdateManager {
    /// 异步检查更新
    ///
    /// 依次尝试 Gitee 和 GitHub 源，返回第一个可用的更新信息
    pub async fn check_update() -> Result<Option<UpdateInfo>> {
        let client = reqwest::Client::builder()
            .user_agent("nrmm-tauri")
            .build()?;

        if let Ok(info) = Self::check_gitee(&client).await {
            let current = env!("CARGO_PKG_VERSION");
            if Self::needs_update(current, &info.version) && !info.download_url.is_empty() {
                return Ok(Some(info));
            }
        }

        if let Ok(info) = Self::check_github(&client).await {
            let current = env!("CARGO_PKG_VERSION");
            if Self::needs_update(current, &info.version) && !info.download_url.is_empty() {
                return Ok(Some(info));
            }
        }

        Ok(None)
    }

    async fn check_gitee(client: &reqwest::Client) -> Result<UpdateInfo> {
        let resp = client.get(GITEE_RELEASES_API).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Gitee API error: {}", resp.status());
        }
        let release: serde_json::Value = resp.json().await?;
        Self::parse_release_info(&release, "gitee")
    }

    async fn check_github(client: &reqwest::Client) -> Result<UpdateInfo> {
        let resp = client.get(GITHUB_RELEASES_API).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("GitHub API error: {}", resp.status());
        }
        let release: serde_json::Value = resp.json().await?;
        Self::parse_release_info(&release, "github")
    }

    fn parse_release_info(release: &serde_json::Value, source: &str) -> Result<UpdateInfo> {
        let tag_name = release["tag_name"].as_str().unwrap_or("0.0.0");
        let version = tag_name.trim_start_matches('v').to_string();
        let body = release["body"].as_str().unwrap_or("").to_string();
        let published = release["published_at"].as_str().unwrap_or("").to_string();

        let mut download_url = String::new();

        if let Some(assets) = release["assets"].as_array() {
            #[cfg(target_os = "windows")]
            let keyword = "windows";
            #[cfg(target_os = "linux")]
            let keyword = "linux";
            #[cfg(target_os = "macos")]
            let keyword = "macos";

            for asset in assets {
                let name = asset["name"].as_str().unwrap_or("").to_lowercase();

                if name.contains(keyword) && (
                    name.ends_with(".msi") ||
                    name.ends_with(".exe") ||
                    name.ends_with(".AppImage") ||
                    name.ends_with(".dmg") ||
                    name.ends_with(".deb") ||
                    name.ends_with(".rpm") ||
                    name.ends_with(".nsis.zip") ||
                    name.ends_with(".msi.zip")
                ) {
                    if source == "gitee" {
                        if let Some(url) = asset["browser_download_url"].as_str() {
                            download_url = url.to_string();
                            break;
                        }
                        if let Some(url) = asset["download_url"].as_str() {
                            download_url = url.to_string();
                            break;
                        }
                        if let Some(url) = asset["url"].as_str() {
                            download_url = url.to_string();
                            break;
                        }
                    } else {
                        if let Some(url) = asset["browser_download_url"].as_str() {
                            download_url = url.to_string();
                            break;
                        }
                    }
                }
            }
        }

        Ok(UpdateInfo {
            version,
            download_url,
            release_notes: body,
            published_at: published,
            source: source.to_string(),
        })
    }

    /// 比较版本号，判断是否需要更新
    ///
    /// # 算法
    /// 按点号分割为数字数组，逐位比较：latest 任一位更大则需要更新
    /// 如果前面都相等，latest 位数更多（如 1.0 vs 1.0.1）也需要更新
    pub fn needs_update(current: &str, latest: &str) -> bool {
        let parse_version = |v: &str| -> Vec<u32> {
            v.trim_start_matches('v')
                .split('.')
                .filter_map(|s| s.parse().ok())
                .collect()
        };

        let current_parts = parse_version(current);
        let latest_parts = parse_version(latest);

        for (c, l) in current_parts.iter().zip(latest_parts.iter()) {
            if l > c { return true; }
            if l < c { return false; }
        }

        latest_parts.len() > current_parts.len()
    }
}

/// 检查更新（Tauri 命令）
#[tauri::command]
pub async fn check_for_updates() -> Result<Option<UpdateInfo>, String> {
    UpdateManager::check_update().await.map_err(|e| e.to_string())
}

/// 比较版本号（Tauri 命令）
#[tauri::command]
pub fn compare_versions(current: String, latest: String) -> bool {
    UpdateManager::needs_update(&current, &latest)
}

/// 获取当前应用版本（Tauri 命令）
#[tauri::command]
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
