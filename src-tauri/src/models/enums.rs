use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum TargetGame {
    GenshinImpact,
    HonkaiStarRail,
    Wuwa,
    ZZZ,
    HonkaiImpact3rd,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GroupType {
    NormalGroup,
    ExclusiveSlot,
    CustomParallel,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModsPathStatus {
    Valid,
    NotFound,
    ManagedFolderNotFound,
    D3dxIniNotFound,
    Normal,
    Empty,
    NoAccess,
    NotSet,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LayoutMode {
    Grid,
    Carousel,
    Automatic,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortingType {
    Default,
    Alphabetical,
    RecentMod,
    ReverseAlphabetical,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CursorType {
    Normal,
    Precision,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
    Success,
}

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
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetGame::GenshinImpact => "GenshinImpact",
            TargetGame::HonkaiStarRail => "HonkaiStarRail",
            TargetGame::Wuwa => "Wuwa",
            TargetGame::ZZZ => "ZZZ",
            TargetGame::HonkaiImpact3rd => "HonkaiImpact3rd",
        }
    }
    
    pub fn display_name(&self) -> &'static str {
        match self {
            TargetGame::GenshinImpact => "Genshin Impact",
            TargetGame::HonkaiStarRail => "Honkai: Star Rail",
            TargetGame::Wuwa => "Wuthering Waves",
            TargetGame::ZZZ => "Zenless Zone Zero",
            TargetGame::HonkaiImpact3rd => "Honkai Impact 3rd",
        }
    }
    
    pub fn process_names(&self) -> &[&str] {
        match self {
            TargetGame::GenshinImpact => &["GenshinImpact.exe", "YuanShen.exe"],
            TargetGame::HonkaiStarRail => &["StarRail.exe"],
            TargetGame::Wuwa => &["Wuthering Waves.exe", "WutheringWaves.exe", "Client-Win64-Shipping.exe"],
            TargetGame::ZZZ => &["ZenlessZoneZero.exe"],
            TargetGame::HonkaiImpact3rd => &["BH3.exe"],
        }
    }
    
    pub fn d3dx_ini_name(&self) -> &'static str {
        match self {
            TargetGame::HonkaiStarRail => "RatioShot.ini",
            _ => "d3dx.ini",
        }
    }
    
    pub fn all() -> [TargetGame; 5] {
        [TargetGame::GenshinImpact, TargetGame::HonkaiStarRail, TargetGame::Wuwa, TargetGame::ZZZ, TargetGame::HonkaiImpact3rd]
    }
}
