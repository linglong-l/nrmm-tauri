//! 数据模型模块
//!
//! 定义应用中使用的所有数据结构：
//! - `enums`: 枚举类型（目标游戏、分组类型、布局模式等）
//! - `mod_data`: 模组相关数据结构（ModData, ModGroupData, ModIniData 等）
//! - `settings`: 应用设置数据结构（AppSettings, HotkeyKeyboard 等）
//!
//! 所有数据结构都支持 serde 序列化/反序列化，用于前后端通信和配置持久化

pub mod enums;
pub mod mod_data;
pub mod settings;

pub use enums::*;
pub use mod_data::*;
pub use settings::*;
