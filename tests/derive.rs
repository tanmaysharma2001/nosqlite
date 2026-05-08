use nosqlite::{document, Database, Document, TypedCollection};
use serde_json::json;

#[document]
#[derive(Debug, Clone, PartialEq)]
struct User {
    #[id]
    user_id: Option<String>,
    name: String,
    age: u32,
    #[serde(default)]
    tags: Vec<String>,
}

#[test]
fn document_macro_round_trips_via_typed_collection() {
    let db = Database::open_in_memory().unwrap();
    let users: TypedCollection<User> = db.typed_collection("users");

    let mut alice = User {
        user_id: None,
        name: "Alice".into(),
        age: 30,
        tags: vec!["admin".into()],
    };
    users.insert(&mut alice).unwrap();
    assert!(alice.user_id.is_some());
    let id = alice.user_id.clone().unwrap();

    // Document trait gives us back the id.
    assert_eq!(<User as Document>::id(&alice), Some(id.as_str()));

    // get() finds by id.
    let fetched: User = users.get(&id).unwrap().unwrap();
    assert_eq!(fetched, alice);
}

#[test]
fn document_macro_falls_back_to_field_named_id() {
    #[document]
    #[derive(Debug)]
    struct Plain {
        id: Option<String>,
        v: i64,
    }

    let db = Database::open_in_memory().unwrap();
    let coll: TypedCollection<Plain> = db.typed_collection("plain");

    let mut p = Plain { id: None, v: 42 };
    coll.insert(&mut p).unwrap();
    assert!(p.id.is_some());
}

#[test]
fn explicit_id_is_preserved_via_typed_insert() {
    let db = Database::open_in_memory().unwrap();
    let users: TypedCollection<User> = db.typed_collection("users");
    let mut u = User {
        user_id: Some("hand-picked".into()),
        name: "Bob".into(),
        age: 22,
        tags: vec![],
    };
    users.insert(&mut u).unwrap();
    assert_eq!(u.user_id.as_deref(), Some("hand-picked"));
    let from_db = users.get("hand-picked").unwrap().unwrap();
    assert_eq!(from_db.name, "Bob");
}

#[test]
fn macro_works_with_filters_and_updates() {
    let db = Database::open_in_memory().unwrap();
    let users: TypedCollection<User> = db.typed_collection("users");
    users
        .insert_many(&[
            User {
                user_id: None,
                name: "A".into(),
                age: 30,
                tags: vec![],
            },
            User {
                user_id: None,
                name: "B".into(),
                age: 22,
                tags: vec![],
            },
        ])
        .unwrap();

    let n = users
        .update_many(json!({}), json!({ "$inc": { "age": 1 } }))
        .unwrap();
    assert_eq!(n, 2);

    let oldest: User = users
        .find(json!({}))
        .sort(json!({ "age": -1 }))
        .limit(1)
        .into_vec()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(oldest.name, "A");
    assert_eq!(oldest.age, 31);
}
