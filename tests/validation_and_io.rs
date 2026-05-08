use nosqlite::{Database, Format, ValidationLevel};
use serde_json::json;

#[test]
fn validator_rejects_bad_inserts() {
    let db = Database::open_in_memory().unwrap();
    let schema = json!({
        "type": "object",
        "required": ["name", "age"],
        "properties": {
            "name": { "type": "string", "minLength": 1 },
            "age":  { "type": "integer", "minimum": 0 }
        },
        "additionalProperties": true
    });
    db.set_validator("users", schema, ValidationLevel::Strict)
        .unwrap();
    let users = db.collection("users");

    users
        .insert_one(json!({ "name": "Alice", "age": 30 }))
        .unwrap();
    assert!(users.insert_one(json!({ "name": "" })).is_err());
    assert!(users
        .insert_one(json!({ "name": "Bob", "age": -1 }))
        .is_err());
    assert!(users.insert_one(json!({ "age": 5 })).is_err());

    assert_eq!(users.count_all().unwrap(), 1);
}

#[test]
fn validator_warn_mode_passes_through() {
    let db = Database::open_in_memory().unwrap();
    db.set_validator(
        "users",
        json!({ "type": "object", "required": ["name"] }),
        ValidationLevel::Warn,
    )
    .unwrap();
    let users = db.collection("users");
    // Should succeed despite missing required field, because level is Warn.
    users.insert_one(json!({ "x": 1 })).unwrap();
    assert_eq!(users.count_all().unwrap(), 1);
}

#[test]
fn validator_returns_validation_failed_variant() {
    let db = Database::open_in_memory().unwrap();
    db.set_validator(
        "users",
        json!({ "type": "object", "required": ["name"] }),
        ValidationLevel::Strict,
    )
    .unwrap();
    let err = db.collection("users").insert_one(json!({})).unwrap_err();
    assert!(
        matches!(err, nosqlite::Error::ValidationFailed(_)),
        "expected ValidationFailed, got {:?}",
        err
    );
}

#[test]
fn validator_runs_on_update() {
    let db = Database::open_in_memory().unwrap();
    db.set_validator(
        "items",
        json!({
            "type": "object",
            "properties": { "qty": { "type": "integer", "minimum": 0 } }
        }),
        ValidationLevel::Strict,
    )
    .unwrap();
    let c = db.collection("items");
    c.insert_one(json!({ "_id": "x", "qty": 5 })).unwrap();
    assert!(c
        .update_one(json!({ "_id": "x" }), json!({ "$set": { "qty": -3 } }))
        .is_err());
    let d = c.find_one(json!({ "_id": "x" })).unwrap().unwrap();
    assert_eq!(d["qty"], 5);
}

#[test]
fn validator_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.nosqlite");
    {
        let db = Database::open(&path).unwrap();
        db.set_validator(
            "c",
            json!({ "type": "object", "required": ["x"] }),
            ValidationLevel::Strict,
        )
        .unwrap();
    }
    let db = Database::open(&path).unwrap();
    let c = db.collection("c");
    assert!(c.insert_one(json!({ "y": 1 })).is_err());
    c.insert_one(json!({ "x": 1 })).unwrap();
}

#[test]
fn import_export_jsonl_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.jsonl");

    let db_a = Database::open_in_memory().unwrap();
    let a = db_a.collection("c");
    a.insert_many(vec![
        json!({ "n": 1, "name": "a" }),
        json!({ "n": 2, "name": "b" }),
        json!({ "n": 3, "name": "c" }),
    ])
    .unwrap();
    let n = a.export_file(&path, Format::Jsonl, json!({})).unwrap();
    assert_eq!(n, 3);

    let db_b = Database::open_in_memory().unwrap();
    let b = db_b.collection("c");
    let imported = b.import_file(&path, Format::Jsonl).unwrap();
    assert_eq!(imported, 3);
    assert_eq!(b.count_all().unwrap(), 3);
    assert_eq!(b.count(json!({ "n": 2 })).unwrap(), 1);
}

#[test]
fn import_export_json_array() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.json");

    let db_a = Database::open_in_memory().unwrap();
    let a = db_a.collection("x");
    a.insert_many(vec![json!({ "v": 1 }), json!({ "v": 2 })])
        .unwrap();
    a.export_file(&path, Format::Json, json!({})).unwrap();

    let db_b = Database::open_in_memory().unwrap();
    let n = db_b
        .collection("x")
        .import_file(&path, Format::Json)
        .unwrap();
    assert_eq!(n, 2);
}

#[test]
fn export_with_filter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("filtered.jsonl");

    let db = Database::open_in_memory().unwrap();
    let c = db.collection("c");
    c.insert_many(vec![
        json!({ "n": 1, "keep": false }),
        json!({ "n": 2, "keep": true }),
        json!({ "n": 3, "keep": true }),
    ])
    .unwrap();

    let n = c
        .export_file(&path, Format::Jsonl, json!({ "keep": true }))
        .unwrap();
    assert_eq!(n, 2);
}
