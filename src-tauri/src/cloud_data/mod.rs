//! 云端数据模块
//!
//! 该模块负责从远程服务器获取 NRMM 的云端配置数据，包括：
//! - 外部链接（GitHub、Discord、Ko-fi 等社区入口）
//! - 公告消息（全局消息及各游戏专属消息）
//! - 自动图标数据（根据纹理 hash 自动匹配角色图标）
//! - 已知模组库命名空间列表（用于错误检测中的库引用校验）
//!
//! 当云端 URL 未配置时，会回退到内置的 mock 数据，保证离线环境下功能正常。

use std::time::Duration;

use anyhow::{Context, Result};
use log::info;
use serde::{Deserialize, Serialize};

use crate::process::TargetGame;

/// 云端数据获取地址。
///
/// 当前为空字符串，表示未配置远程地址，此时 `fetch` 会返回内置的 mock 数据。
/// 部署时可通过修改此常量指向实际的云端 JSON 接口。
const CLOUD_DATA_URL: &str = "";

/// HTTP 请求默认超时时间（秒）。
///
/// 用于防止网络异常时请求长时间挂起，默认 10 秒。
const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// 云端数据聚合结构体。
///
/// 包含 NRMM 所需的全部云端配置，通过 `serde` 序列化/反序列化为 JSON。
/// 实现了 `Default`，在远程获取失败或离线时可作为空数据使用。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CloudData {
    /// 外部社区链接（GitHub、Discord、Ko-fi）。
    pub links: CloudLinks,
    /// 公告消息（全局 + 各游戏专属）。
    pub messages: CloudMessages,
    /// 自动图标匹配数据列表（纹理 hash → 角色信息）。
    pub auto_icons: Vec<AutoIconData>,
    /// 已知模组库命名空间列表（用于 INI 错误检测中的库引用校验）。
    pub known_mod_libraries: Vec<String>,
}

/// 外部社区链接集合。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CloudLinks {
    /// GitHub 仓库地址。
    pub github: String,
    /// Discord 社区邀请链接。
    pub discord: String,
    /// Ko-fi 赞助页面链接。
    pub kofi: String,
}

/// 公告消息集合。
///
/// 支持全局消息和按游戏分类的专属消息。
/// 各游戏的专属消息为 `Option`，未配置时回退到全局消息（见 `get_message_for_game`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CloudMessages {
    /// 全局公告消息（所有游戏共用）。
    pub global: Option<String>,
    /// 鸣潮专属消息。
    pub wuthering_waves: Option<String>,
    /// 原神专属消息。
    pub genshin_impact: Option<String>,
    /// 崩坏：星穹铁道专属消息。
    pub honkai_star_rail: Option<String>,
    /// 绝区零专属消息。
    pub zenless_zone_zero: Option<String>,
    /// 明日方舟：终末地专属消息。
    pub arknights_endfield: Option<String>,
}

/// 自动图标匹配数据。
///
/// 描述单个纹理 hash 与角色名称、所属游戏的映射关系，
/// 用于在扫描模组时自动为未设置图标的模组匹配角色头像。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoIconData {
    /// 纹理资源的 hash 值（3DMigoto 中 `hash = 0x...` 形式）。
    pub hash: String,
    /// 对应的角色名称（用于图标文件名匹配）。
    pub character_name: String,
    /// 所属游戏标识字符串。
    pub game: String,
}

impl CloudData {
    /// 创建一个空的 `CloudData` 实例（所有字段为默认值）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用默认超时时间（10 秒）获取云端数据。
    ///
    /// 返回：解析后的 `CloudData`；URL 未配置时返回 mock 数据。
    pub async fn fetch() -> Result<Self> {
        Self::fetch_with_timeout(DEFAULT_TIMEOUT_SECS).await
    }

    /// 使用指定超时时间获取云端数据。
    ///
    /// 流程：
    /// 1. 若 `CLOUD_DATA_URL` 为空，返回内置 mock 数据（离线模式）。
    /// 2. 创建带超时设置的 HTTP 客户端。
    /// 3. 发送 GET 请求并校验响应状态码。
    /// 4. 将响应体解析为 `CloudData` JSON。
    ///
    /// 参数：
    /// - `timeout_secs`: HTTP 请求超时时间（秒）。
    ///
    /// 返回：解析后的 `CloudData`。
    /// 错误：客户端创建失败、请求失败、状态码非 2xx、JSON 解析失败时返回 `anyhow::Error`。
    pub async fn fetch_with_timeout(timeout_secs: u64) -> Result<Self> {
        // URL 未配置时使用 mock 数据，保证离线可用
        if CLOUD_DATA_URL.is_empty() {
            info!("Cloud data URL not configured, using mock data");
            return Ok(Self::mock_data());
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .context("Failed to create HTTP client")?;

        let response = client
            .get(CLOUD_DATA_URL)
            .send()
            .await
            .with_context(|| format!("Failed to fetch cloud data from {}", CLOUD_DATA_URL))?;

        // 校验 HTTP 状态码
        if !response.status().is_success() {
            anyhow::bail!(
                "Cloud data request failed with status: {}",
                response.status()
            );
        }

        let cloud_data: CloudData = response
            .json()
            .await
            .context("Failed to parse cloud data JSON")?;

        info!("Cloud data fetched successfully");
        Ok(cloud_data)
    }

    /// 获取指定游戏的公告消息。
    ///
    /// 优先返回该游戏的专属消息；若专属消息未配置（`None`），则回退到全局消息。
    /// `TargetGame::None` 直接返回全局消息。
    ///
    /// 参数：
    /// - `game`: 目标游戏枚举值。
    ///
    /// 返回：对应的公告消息字符串引用（可能为 `None`）。
    #[allow(dead_code)]
    pub fn get_message_for_game(&self, game: TargetGame) -> Option<&str> {
        match game {
            TargetGame::None => self.messages.global.as_deref(),
            TargetGame::WutheringWaves => self
                .messages
                .wuthering_waves
                .as_deref()
                .or(self.messages.global.as_deref()),
            TargetGame::GenshinImpact => self
                .messages
                .genshin_impact
                .as_deref()
                .or(self.messages.global.as_deref()),
            TargetGame::HonkaiStarRail => self
                .messages
                .honkai_star_rail
                .as_deref()
                .or(self.messages.global.as_deref()),
            TargetGame::ZenlessZoneZero => self
                .messages
                .zenless_zone_zero
                .as_deref()
                .or(self.messages.global.as_deref()),
            TargetGame::ArknightsEndfield => self
                .messages
                .arknights_endfield
                .as_deref()
                .or(self.messages.global.as_deref()),
        }
    }

    /// 根据纹理 hash 查找匹配的自动图标数据。
    ///
    /// 参数：
    /// - `hash`: 纹理 hash 字符串。
    ///
    /// 返回：匹配到的 `AutoIconData` 引用（可能为 `None`）。
    #[allow(dead_code)]
    pub fn get_auto_icon_for_hash(&self, hash: &str) -> Option<&AutoIconData> {
        self.auto_icons.iter().find(|icon| icon.hash == hash)
    }

    /// 判断指定名称是否为已知的模组库。
    ///
    /// 匹配不区分大小写。
    ///
    /// 参数：
    /// - `lib_name`: 待检测的库名称。
    ///
    /// 返回：是已知库返回 `true`，否则返回 `false`。
    #[allow(dead_code)]
    pub fn is_known_library(&self, lib_name: &str) -> bool {
        self.known_mod_libraries
            .iter()
            .any(|lib| lib.eq_ignore_ascii_case(lib_name))
    }

    /// 生成内置的 mock 数据（离线模式使用）。
    ///
    /// 提供基本的链接和欢迎消息，确保未配置云端 URL 时前端仍有数据可展示。
    fn mock_data() -> Self {
        Self {
            links: CloudLinks {
                github: "https://github.com/".to_string(),
                discord: "https://discord.gg/".to_string(),
                kofi: "https://ko-fi.com/".to_string(),
            },
            messages: CloudMessages {
                global: Some("Welcome to XXMI-NRMM!".to_string()),
                wuthering_waves: None,
                genshin_impact: None,
                honkai_star_rail: None,
                zenless_zone_zero: None,
                arknights_endfield: None,
            },
            auto_icons: vec![],
            known_mod_libraries: vec![
                "XXMI".to_string(),
                "NRMM".to_string(),
            ],
        }
    }
}
