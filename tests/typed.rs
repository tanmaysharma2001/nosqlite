use nosqlite::{Database, TypedCollection};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct User {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    age: u32,
    #[serde(default)]
    tags: Vec<String>,
}

#[test]
fn typed_round_trip() {
    let db = Database::open_in_memory().unwrap();
    let users: TypedCollection<User> = db.typed_collection("users");

    let id = users
        .insert_one(&User {
            id: None,
            name: "Alice".into(),
            age: 30,
            tags: vec!["admin".into()],
        })
        .unwrap();

    let alice: User = users.find_one(json!({ "_id": &id })).unwrap().unwrap();

    assert_eq!(alice.id.as_deref(), Some(id.as_str()));
    assert_eq!(alice.name, "Alice");
    assert_eq!(alice.age, 30);
    assert_eq!(alice.tags, vec!["admin".to_string()]);
}

#[test]
fn typed_find_cursor_sort_limit() {
    let db = Database::open_in_memory().unwrap();
    let users: TypedCollection<User> = db.typed_collection("users");
    users
        .insert_many(&[
            User {
                id: None,
                name: "Alice".into(),
                age: 30,
                tags: vec![],
            },
            User {
                id: None,
                name: "Bob".into(),
                age: 22,
                tags: vec![],
            },
            User {
                id: None,
                name: "Carol".into(),
                age: 41,
                tags: vec![],
            },
        ])
        .unwrap();

    let oldest_two: Vec<User> = users
        .find(json!({}))
        .sort(json!({ "age": -1 }))
        .limit(2)
        .into_vec()
        .unwrap();

    assert_eq!(oldest_two.len(), 2);
    assert_eq!(oldest_two[0].name, "Carol");
    assert_eq!(oldest_two[1].name, "Alice");
}

#[test]
fn typed_replace_and_update_through_value() {
    let db = Database::open_in_memory().unwrap();
    let users: TypedCollection<User> = db.typed_collection("users");
    let id = users
        .insert_one(&User {
            id: None,
            name: "old".into(),
            age: 1,
            tags: vec![],
        })
        .unwrap();

    // Filters and updates remain JSON for full MQL access.
    users
        .update_one(json!({ "_id": &id }), json!({ "$inc": { "age": 5 } }))
        .unwrap();

    users
        .replace_one(
            json!({ "_id": &id }),
            &User {
                id: None,
                name: "new".into(),
                age: 99,
                tags: vec!["x".into()],
            },
        )
        .unwrap();

    let after: User = users.find_one(json!({ "_id": &id })).unwrap().unwrap();
    assert_eq!(after.name, "new");
    assert_eq!(after.age, 99);
    assert_eq!(after.tags, vec!["x".to_string()]);
}

#[test]
fn typed_can_drop_to_untyped() {
    let db = Database::open_in_memory().unwrap();
    let users: TypedCollection<User> = db.typed_collection("users");
    users
        .insert_one(&User {
            id: None,
            name: "Alice".into(),
            age: 30,
            tags: vec![],
        })
        .unwrap();
    // Indexes / aggregation / FTS still operate on the underlying Collection.
    users.untyped().create_index(json!({ "age": 1 })).unwrap();
    let plan = users
        .untyped()
        .find(json!({ "age": 30 }))
        .explain()
        .unwrap();
    assert!(plan.rows.iter().any(|r| r.detail.contains("USING INDEX")));
}
