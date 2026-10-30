use super::*;

/// No door is decided under a name the exposure policy never holds: a door so named would pass every rule unheld.
#[test]
fn no_door_operation_is_an_exempt_name() {
    for op in DOOR_OPERATIONS {
        assert!(!EXEMPT_OPERATIONS.contains(op), "{op} is an exempt operation");
    }
}

/// A Rust verb's causal fields stand for the body a gateway route reads: `_meta` with whichever field is set, nothing
/// when neither is — so an untouched door's decision would see what a root request carries.
#[test]
fn a_verbs_causal_fields_are_the_bodys_meta() {
    let both = CausalFields { correlation: Some("run-1".into()), parent: Some("ab".repeat(32)) };
    assert_eq!(params_of(&both), serde_json::json!({ "_meta": { "correlation": "run-1", "parent": "ab".repeat(32) } }));
    assert_eq!(params_of(&CausalFields::default()), serde_json::json!({}));
    assert_eq!((point(true, "tuple/take"), point(false, "tuple/take")), ("gateway:tuple/take".to_string(), "node:tuple/take".to_string()));
}
