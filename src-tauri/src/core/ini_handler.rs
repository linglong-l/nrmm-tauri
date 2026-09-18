// 过渡门面（试点 3.7 收口时随消费方切换删除）
pub use crate::core::ini_codec::{
    read_ini_safe, write_utf8_atomic, SourceEncoding, SourceEncodingInfo,
};
// 过渡门面（试点 3.7 收口时随消费方切换删除）
pub use crate::core::ini_condition::{
    is_conditional_section, is_key_section, sanitize_condition_expression_public,
};
// 过渡门面（试点 3.7 收口时随消费方切换删除）
pub use crate::core::ini_model::{IniFile, IniLine, IniSection};
