#![cfg(feature = "bson")]

use nosqlite::Database;
use serde_json::json;
use std::io::Write;

#[test]
fn import_bson_documents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.bson");

    // Build a small mongodump-style file: concatenated BSON documents.
    let docs = vec![
        bson::doc! { "_id": "u1", "name": "Alice", "age": 30, "tags": ["admin", "owner"] },
        bson::doc! { "_id": "u2", "name": "Bob",   "age": 22 },
        bson::doc! { "_id": "u3", "name": "Carol", "addr": { "city": "NYC" } },
    ];
    let mut f = std::fs::File::create(&path).unwrap();
    for d in &docs {
        let bytes = bson::to_vec(d).unwrap();
        f.write_all(&bytes).unwrap();
    }
    drop(f);

    let db = Database::open_in_memory().unwrap();
    let users = db.collection("users");
    let n = users.import_bson_file(&path).unwrap();
    assert_eq!(n, 3);

    // Round-trip via filter + projection.
    let alice = users.find_one(json!({ "_id": "u1" })).unwrap().unwrap();
    assert_eq!(alice["name"], "Alice");
    assert_eq!(alice["tags"], json!(["admin", "owner"]));

    let in_nyc = users.count(json!({ "addr.city": "NYC" })).unwrap();
    assert_eq!(in_nyc, 1);
}

#[test]
fn bson_object_id_becomes_hex_string() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oid.bson");

    let oid = bson::oid::ObjectId::new();
    let doc = bson::doc! { "_id": oid, "name": "x" };
    std::fs::write(&path, bson::to_vec(&doc).unwrap()).unwrap();

    let db = Database::open_in_memory().unwrap();
    let c = db.collection("c");
    c.import_bson_file(&path).unwrap();

    // The ObjectId should have been mapped to its hex form.
    let r = c.find_one(json!({ "_id": oid.to_hex() })).unwrap().unwrap();
    assert_eq!(r["name"], "x");
}
