//! 设置对象的 IPC 写入边界
//!
//! # 为什么需要它
//! `save_settings` 命令此前接收**完整** `AppSettings`，而 `AppSettings.game_mods_path`
//! 正是路径校验的锚点：同一次调用既能把某个游戏的 Mods 根改成 `C:\` 或用户主目录，
//! 又依赖这张表判定「是否越界」——校验者可被自己校验的对象改写，整条 R1 防线形同虚设。
//!
//! # 做法
//! 不复制二十多个字段（那会带来「两个结构漂移、新字段静默丢失」的更差故障模式），
//! 而是在边界上包一层：反序列化后把 `game_mods_path` 强制还原为服务端当前值，
//! 并把「前端试图改写锚点」记为审计日志。改写 Mods 根的唯一入口变成
//! `commands::settings_commands::set_game_mods_path` 这一条专用命令——它做目录校验、
//! 登记锚点、并只更新该字段。

use serde::Deserialize;

use crate::config::settings_store;
use crate::models::settings::AppSettings;

/// `save_settings` 的可写入参：整份设置对象，但 `game_mods_path` 一律以服务端现值为准。
///
/// # 不变量
/// 构造成功后 `settings.game_mods_path` 恒等于 [`settings_store::get_settings`] 的当前值，
/// 因此前端无法经由本类型改写路径校验锚点。
#[derive(Debug, Clone)]
pub struct AppSettingsWritable(pub AppSettings);

impl AppSettingsWritable {
    /// 交出内部设置对象（锚点字段已被归一）
    pub fn into_inner(self) -> AppSettings {
        self.0
    }

    /// 被忽略的锚点改写尝试数量（供审计与测试断言）
    ///
    /// # 参数
    /// - `incoming`: 前端提交的设置
    /// - `current`: 服务端当前设置
    fn audit_anchor_write(incoming: &AppSettings, current: &AppSettings) {
        if incoming.game_mods_path == current.game_mods_path {
            return;
        }
        // 只记录键集合与数量，不落全量路径（前端可控制的内容进日志需谨慎）
        log::warn!(
            "[AUDIT] save_settings 试图改写校验锚点 game_mods_path，已忽略：\
             提交 {} 项 / 现有 {} 项",
            incoming.game_mods_path.len(),
            current.game_mods_path.len()
        );
    }
}

impl<'de> Deserialize<'de> for AppSettingsWritable {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let incoming = AppSettings::deserialize(deserializer)?;
        let current = settings_store::get_settings();
        Self::audit_anchor_write(&incoming, &current);
        Ok(Self(AppSettings {
            game_mods_path: current.game_mods_path,
            ..incoming
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::TargetGame;

    /// 锚点归一：提交里带越界根（`C:\` / 主目录）也必须被服务端现值覆盖
    #[test]
    fn writable_dto_overrides_anchor_field() {
        let current = settings_store::get_settings();
        let mut submitted = current.clone();
        submitted
            .game_mods_path
            .insert(TargetGame::GenshinImpact, "C:\\".to_string());

        let json = serde_json::to_string(&submitted).unwrap();
        let parsed: AppSettingsWritable = serde_json::from_str(&json).unwrap();

        assert_eq!(
            parsed.0.game_mods_path, current.game_mods_path,
            "game_mods_path 必须取服务端现值，不接受前端提交"
        );
    }

    /// 其余字段照常生效（证明归一没有顺手吃掉别的设置）
    #[test]
    fn writable_dto_keeps_other_fields() {
        let current = settings_store::get_settings();
        let mut submitted = current.clone();
        submitted.language = "en".to_string();
        submitted.interface_scale = 1.25;

        let json = serde_json::to_string(&submitted).unwrap();
        let parsed: AppSettingsWritable = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.0.language, "en");
        assert_eq!(parsed.0.interface_scale, 1.25);
    }
}
