use super::*;

/// 契约锁定：managed-partial-update payload 序列化为 { game, deltas }（camelCase，
/// 组内复用 GroupDelta 契约）
#[test]
fn managed_partial_update_payload_serializes_camel_case() {
    let payload = ManagedPartialUpdatePayload {
        game: "GenshinImpact".to_string(),
        deltas: vec![GroupDelta {
            old_group_path: "D:/Mods/_MANAGED_/group_1".to_string(),
            new_group: None,
            old_mod_paths: vec!["D:/Mods/_MANAGED_/group_1/ModA".to_string()],
        }],
    };
    let v = serde_json::to_value(&payload).expect("payload 序列化失败");
    assert!(v.get("game").is_some(), "应含 game 字段");
    let deltas = v
        .get("deltas")
        .and_then(serde_json::Value::as_array)
        .expect("deltas 应为数组");
    assert_eq!(deltas.len(), 1, "应携带 1 个 delta");
    assert!(
        deltas[0].get("oldGroupPath").is_some(),
        "组内 delta 字段应为 camelCase（复用 GroupDelta 契约）"
    );
}
