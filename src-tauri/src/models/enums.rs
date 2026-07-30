//! 枚举类型定义模块
//!
//! 包含应用中使用的所有枚举类型，均支持 serde 序列化（camelCase 命名）

use serde::{Serialize, Deserialize};

/// 支持的目标游戏列表
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum TargetGame {
    GenshinImpact,
    HonkaiStarRail,
    Wuwa,
    ZZZ,
    HonkaiImpact3rd,
    ArknightsEndfield,
}

/// 模组分组类型
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GroupType {
    /// 普通分组（group_xx，一级子目录互斥）
    NormalGroup,
    ExclusiveSlot,
    CustomParallel,
    /// 互斥组（支持任意深度嵌套，同级互斥）
    MutexGroup,
}

/// 模组路径状态
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModsPathStatus {
    /// 路径有效
    Valid,
    /// 路径不存在
    NotFound,
    /// _MANAGED_ 目录不存在
    ManagedFolderNotFound,
    /// 主 INI 文件不存在
    D3dxIniNotFound,
    Normal,
    Empty,
    NoAccess,
    NotSet,
}

/// UI 布局模式
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LayoutMode {
    /// 网格布局
    Grid,
    /// 轮播布局
    Carousel,
    /// 自动选择
    Automatic,
}

/// 排序方式
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortingType {
    /// 默认排序
    Default,
    /// 字母顺序
    Alphabetical,
    /// 最近使用
    RecentMod,
    /// 反向字母顺序
    ReverseAlphabetical,
}

/// 光标类型
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CursorType {
    Normal,
    Precision,
}

/// 通知级别
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
    Success,
}

/// 按键配置方案
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum KeybindProfile {
    GI,
    HSR,
    Wuwa,
    ZZZ,
    Hi3,
}

impl TargetGame {
    /// 获取游戏标识字符串（用于路径、缓存 key 等）
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetGame::GenshinImpact => "GenshinImpact",
            TargetGame::HonkaiStarRail => "HonkaiStarRail",
            TargetGame::Wuwa => "Wuwa",
            TargetGame::ZZZ => "ZZZ",
            TargetGame::HonkaiImpact3rd => "HonkaiImpact3rd",
            TargetGame::ArknightsEndfield => "ArknightsEndfield",
        }
    }
    
    /// 获取游戏显示名称（用于 UI）
    pub fn display_name(&self) -> &'static str {
        match self {
            TargetGame::GenshinImpact => "Genshin Impact",
            TargetGame::HonkaiStarRail => "Honkai: Star Rail",
            TargetGame::Wuwa => "Wuthering Waves",
            TargetGame::ZZZ => "Zenless Zone Zero",
            TargetGame::HonkaiImpact3rd => "崩坏三",
            TargetGame::ArknightsEndfield => "Arknights: Endfield",
        }
    }
    
    /// 获取游戏进程名列表（用于前台检测）
    pub fn process_names(&self) -> &[&str] {
        match self {
            TargetGame::GenshinImpact => &["GenshinImpact.exe", "YuanShen.exe"],
            TargetGame::HonkaiStarRail => &["StarRail.exe"],
            TargetGame::Wuwa => &["Wuthering Waves.exe", "WutheringWaves.exe", "Client-Win64-Shipping.exe"],
            TargetGame::ZZZ => &["ZenlessZoneZero.exe"],
            TargetGame::HonkaiImpact3rd => &["BH3.exe"],
            TargetGame::ArknightsEndfield => &["ArknightsEndfield.exe", "Endfield.exe", "Endfield-Win64-Shipping.exe"],
        }
    }
    
    /// 获取主 INI 文件名
    /// 星穹铁道使用 RatioShot.ini，其他游戏使用 d3dx.ini
    pub fn d3dx_ini_name(&self) -> &'static str {
        match self {
            TargetGame::HonkaiStarRail => "RatioShot.ini",
            _ => "d3dx.ini",
        }
    }
    
    /// 获取所有支持的游戏列表
    pub fn all() -> [TargetGame; 6] {
        [TargetGame::GenshinImpact, TargetGame::HonkaiStarRail, TargetGame::Wuwa, TargetGame::ZZZ, TargetGame::HonkaiImpact3rd, TargetGame::ArknightsEndfield]
    }
}
