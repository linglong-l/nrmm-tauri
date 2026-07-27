use anyhow::Result;
use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};
use crate::config::app_paths;

static GITEE_RAW_BASE: &str = "https://gitee.com/Yezi26/nrmm-tauri/raw/master";
static GITHUB_RAW_BASE: &str = "https://raw.githubusercontent.com/linglong-l/nrmm-tauri/master";

pub static CLOUD_DATA_FILES: &[&str] = &[
    "links.json",
    "messages.json",
    "auto_icons.json",
    "known_libraries.json",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudDataInfo {
    pub file_name: String,
    pub gitee_url: String,
    pub github_url: String,
    pub local_path: PathBuf,
    pub last_updated: Option<i64>,
}

pub struct CloudDataManager;

impl CloudDataManager {
    fn cache_dir() -> Result<PathBuf> {
        let dir = app_paths::cloud_cache_dir();
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn cache_path(file_name: &str) -> Result<PathBuf> {
        Ok(Self::cache_dir()?.join(file_name))
    }

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

    pub fn load_data<T: serde::de::DeserializeOwned>(file_name: &str) -> Result<T> {
        let cache_path = Self::cache_path(file_name)?;

        if cache_path.exists() {
            let content = fs::read_to_string(&cache_path)?;
            if let Ok(data) = serde_json::from_str::<T>(&content) {
                return Ok(data);
            }
        }

        if let Some(bundled) = Self::bundled_data_path(file_name) {
            let content = fs::read_to_string(&bundled)?;
            if let Ok(data) = serde_json::from_str::<T>(&content) {
                let _ = fs::write(&cache_path, &content);
                return Ok(data);
            }
        }

        match file_name {
            "links.json" => Ok(serde_json::from_slice(crate::resources::LINKS_JSON)?),
            "messages.json" => Ok(serde_json::from_slice(crate::resources::MESSAGES_JSON)?),
            "auto_icons.json" => Ok(serde_json::from_slice(crate::resources::AUTO_ICONS_JSON)?),
            "known_libraries.json" => Ok(serde_json::from_slice(crate::resources::KNOWN_LIBRARIES_JSON)?),
            _ => anyhow::bail!("Unknown data file: {}", file_name),
        }
    }

    pub async fn refresh_async(file_name: &str) -> Result<()> {
        let gitee_url = format!("{}/src-tauri/src/resources/data/{}", GITEE_RAW_BASE, file_name);
        let github_url = format!("{}/src-tauri/src/resources/data/{}", GITHUB_RAW_BASE, file_name);
        let cache_path = Self::cache_path(file_name)?;

        let client = reqwest::Client::new();

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

        let _ = serde_json::from_str::<serde_json::Value>(&text)?;

        fs::write(&cache_path, text)?;

        log::info!("Cloud data refreshed: {}", file_name);
        Ok(())
    }

    pub async fn refresh_all_async() -> Vec<(String, Result<()>)> {
        let mut results = Vec::new();
        for file in CLOUD_DATA_FILES {
            let result = Self::refresh_async(file).await;
            results.push((file.to_string(), result));
        }
        results
    }
}

#[tauri::command]
pub async fn refresh_cloud_data(file_name: String) -> Result<(), String> {
    CloudDataManager::refresh_async(&file_name).await.map_err(|e| e.to_string())
}

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
