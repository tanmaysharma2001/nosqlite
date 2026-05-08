use nosqlite::Database;
use serde_json::{json, Value};

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

#[test]
fn insert_and_find_one() {
    let db = db();
    let users = db.collection("users");
    let id = users
        .insert_one(json!({ "name": "Alice", "age": 30 }))
        .unwrap();
    assert!(!id.is_empty());

    let found = users.find_one(json!({ "name": "Alice" })).unwrap().unwrap();
    assert_eq!(found["name"], "Alice");
    assert_eq!(found["age"], 30);
    assert_eq!(found["_id"], id);
}

#[test]
fn insert_many_and_count() {
    let db = db();
    let c = db.collection("items");
    let ids = c
        .insert_many(vec![
            json!({ "v": 1 }),
            json!({ "v": 2 }),
            json!({ "v": 3 }),
        ])
        .unwrap();
    assert_eq!(ids.len(), 3);
    assert_eq!(c.count_all().unwrap(), 3);
}

#[test]
fn explicit_id_is_preserved() {
    let db = db();
    let c = db.collection("c");
    let id = c
        .insert_one(json!({ "_id": "user-1", "name": "Bob" }))
        .unwrap();
    assert_eq!(id, "user-1");
    let f = c.find_one(json!({ "_id": "user-1" })).unwrap().unwrap();
    assert_eq!(f["name"], "Bob");
}

#[test]
fn comparison_operators() {
    let db = db();
    let c = db.collection("nums");
    for v in [1, 2, 3, 4, 5] {
        c.insert_one(json!({ "v": v })).unwrap();
    }

    assert_eq!(c.count(json!({ "v": { "$gt": 2 } })).unwrap(), 3);
    assert_eq!(c.count(json!({ "v": { "$gte": 2 } })).unwrap(), 4);
    assert_eq!(c.count(json!({ "v": { "$lt": 3 } })).unwrap(), 2);
    assert_eq!(c.count(json!({ "v": { "$lte": 3 } })).unwrap(), 3);
    assert_eq!(c.count(json!({ "v": { "$ne": 3 } })).unwrap(), 4);
    assert_eq!(c.count(json!({ "v": { "$gt": 1, "$lt": 5 } })).unwrap(), 3);
}

#[test]
fn in_and_nin() {
    let db = db();
    let c = db.collection("x");
    c.insert_many(vec![
        json!({ "color": "red" }),
        json!({ "color": "green" }),
        json!({ "color": "blue" }),
        json!({}),
    ])
    .unwrap();
    assert_eq!(
        c.count(json!({ "color": { "$in": ["red", "blue"] } }))
            .unwrap(),
        2
    );
    assert_eq!(c.count(json!({ "color": { "$nin": ["red"] } })).unwrap(), 3);
}

#[test]
fn exists() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![json!({ "a": 1 }), json!({ "b": 2 })])
        .unwrap();
    assert_eq!(c.count(json!({ "a": { "$exists": true } })).unwrap(), 1);
    assert_eq!(c.count(json!({ "a": { "$exists": false } })).unwrap(), 1);
}

#[test]
fn logical_and_or_nor_not() {
    let db = db();
    let c = db.collection("p");
    c.insert_many(vec![
        json!({ "tag": "a", "n": 1 }),
        json!({ "tag": "b", "n": 2 }),
        json!({ "tag": "c", "n": 3 }),
    ])
    .unwrap();

    let n = c.count(json!({ "$or": [{"n": 1}, {"n": 3}] })).unwrap();
    assert_eq!(n, 2);

    let n = c
        .count(json!({ "$and": [{"n": {"$gt": 1}}, {"tag": "b"}] }))
        .unwrap();
    assert_eq!(n, 1);

    let n = c.count(json!({ "$nor": [{"n": 1}, {"n": 2}] })).unwrap();
    assert_eq!(n, 1);

    let n = c.count(json!({ "n": { "$not": { "$gt": 2 } } })).unwrap();
    assert_eq!(n, 2);
}

#[test]
fn nested_field_match() {
    let db = db();
    let c = db.collection("nest");
    c.insert_one(json!({ "addr": { "city": "NYC", "zip": "10001" }, "name": "A" }))
        .unwrap();
    c.insert_one(json!({ "addr": { "city": "SF" }, "name": "B" }))
        .unwrap();

    let r = c.find_one(json!({ "addr.city": "NYC" })).unwrap().unwrap();
    assert_eq!(r["name"], "A");
    assert_eq!(c.count(json!({ "addr.city": "SF" })).unwrap(), 1);
}

#[test]
fn sort_limit_skip() {
    let db = db();
    let c = db.collection("s");
    for v in [3, 1, 4, 1, 5, 9, 2, 6] {
        c.insert_one(json!({ "v": v })).unwrap();
    }
    let asc = c
        .find(json!({}))
        .sort(json!({ "v": 1 }))
        .into_vec()
        .unwrap();
    let vs: Vec<i64> = asc.iter().map(|d| d["v"].as_i64().unwrap()).collect();
    assert_eq!(vs, vec![1, 1, 2, 3, 4, 5, 6, 9]);

    let top3 = c
        .find(json!({}))
        .sort(json!({ "v": -1 }))
        .limit(3)
        .into_vec()
        .unwrap();
    let vs: Vec<i64> = top3.iter().map(|d| d["v"].as_i64().unwrap()).collect();
    assert_eq!(vs, vec![9, 6, 5]);

    let skipped = c
        .find(json!({}))
        .sort(json!({ "v": 1 }))
        .skip(2)
        .limit(2)
        .into_vec()
        .unwrap();
    let vs: Vec<i64> = skipped.iter().map(|d| d["v"].as_i64().unwrap()).collect();
    assert_eq!(vs, vec![2, 3]);
}

#[test]
fn projection_include_and_exclude() {
    let db = db();
    let c = db.collection("u");
    c.insert_one(json!({ "name": "A", "age": 1, "secret": "s" }))
        .unwrap();

    let r = c
        .find(json!({}))
        .project(json!({ "name": 1, "_id": 0 }))
        .into_vec()
        .unwrap();
    let obj = r[0].as_object().unwrap();
    assert!(obj.contains_key("name"));
    assert!(!obj.contains_key("_id"));
    assert!(!obj.contains_key("age"));

    let r = c
        .find(json!({}))
        .project(json!({ "secret": 0 }))
        .into_vec()
        .unwrap();
    let obj = r[0].as_object().unwrap();
    assert!(obj.contains_key("name"));
    assert!(obj.contains_key("age"));
    assert!(!obj.contains_key("secret"));
}

#[test]
fn nested_projection() {
    let db = db();
    let c = db.collection("p");
    c.insert_one(json!({ "addr": { "city": "NYC", "zip": "10001" }, "n": 1 }))
        .unwrap();
    let r = c
        .find(json!({}))
        .project(json!({ "addr.city": 1, "_id": 0 }))
        .into_vec()
        .unwrap();
    assert_eq!(r[0], json!({ "addr": { "city": "NYC" } }));
}

#[test]
fn update_set_unset_inc() {
    let db = db();
    let c = db.collection("u");
    c.insert_one(json!({ "_id": "x", "n": 1, "stale": true }))
        .unwrap();
    let n = c
        .update_one(
            json!({ "_id": "x" }),
            json!({ "$set": { "n": 5, "tag": "ok" }, "$unset": { "stale": "" } }),
        )
        .unwrap();
    assert_eq!(n, 1);
    let d = c.find_one(json!({ "_id": "x" })).unwrap().unwrap();
    assert_eq!(d["n"], 5);
    assert_eq!(d["tag"], "ok");
    assert!(d.get("stale").is_none());

    c.update_one(json!({ "_id": "x" }), json!({ "$inc": { "n": 3 } }))
        .unwrap();
    let d = c.find_one(json!({ "_id": "x" })).unwrap().unwrap();
    assert_eq!(d["n"], 8);
}

#[test]
fn update_push_pull_rename() {
    let db = db();
    let c = db.collection("u");
    c.insert_one(json!({ "_id": "x", "tags": ["a"] })).unwrap();
    c.update_one(json!({ "_id": "x" }), json!({ "$push": { "tags": "b" } }))
        .unwrap();
    c.update_one(
        json!({ "_id": "x" }),
        json!({ "$push": { "tags": { "$each": ["c", "d"] } } }),
    )
    .unwrap();
    let d = c.find_one(json!({ "_id": "x" })).unwrap().unwrap();
    assert_eq!(d["tags"], json!(["a", "b", "c", "d"]));

    c.update_one(json!({ "_id": "x" }), json!({ "$pull": { "tags": "b" } }))
        .unwrap();
    let d = c.find_one(json!({ "_id": "x" })).unwrap().unwrap();
    assert_eq!(d["tags"], json!(["a", "c", "d"]));

    c.update_one(
        json!({ "_id": "x" }),
        json!({ "$rename": { "tags": "labels" } }),
    )
    .unwrap();
    let d = c.find_one(json!({ "_id": "x" })).unwrap().unwrap();
    assert!(d.get("tags").is_none());
    assert_eq!(d["labels"], json!(["a", "c", "d"]));
}

#[test]
fn replace_one() {
    let db = db();
    let c = db.collection("r");
    let id = c.insert_one(json!({ "name": "old" })).unwrap();
    c.replace_one(json!({ "_id": &id }), json!({ "name": "new", "v": 1 }))
        .unwrap();
    let d = c.find_one(json!({ "_id": &id })).unwrap().unwrap();
    assert_eq!(d["name"], "new");
    assert_eq!(d["v"], 1);
    assert_eq!(d["_id"], id);
}

#[test]
fn delete_one_and_many() {
    let db = db();
    let c = db.collection("d");
    for v in 0..5 {
        c.insert_one(json!({ "v": v })).unwrap();
    }
    let n = c.delete_one(json!({ "v": { "$gte": 0 } })).unwrap();
    assert_eq!(n, 1);
    assert_eq!(c.count_all().unwrap(), 4);

    let n = c.delete_many(json!({ "v": { "$gte": 2 } })).unwrap();
    assert_eq!(n, 3);
    assert_eq!(c.count_all().unwrap(), 1);
}

#[test]
fn create_and_use_indexes() {
    let db = db();
    let c = db.collection("idx");
    for i in 0..200 {
        c.insert_one(json!({ "i": i, "g": i % 5 })).unwrap();
    }
    let name = c.create_index(json!({ "i": 1 })).unwrap();
    assert!(name.contains("nsl_idx"));

    let _ = c
        .create_index_with_options(
            json!({ "g": 1, "i": -1 }),
            Some(json!({ "name": "g_i_idx" })),
        )
        .unwrap();

    let idxs = c.list_indexes().unwrap();
    assert!(idxs.iter().any(|i| i.name == "g_i_idx"));

    // Confirm the planner picks up the index for an indexed lookup.
    let plan = c.find(json!({ "i": 100 })).explain().unwrap();
    let used_index = plan.rows.iter().any(|r| r.detail.contains("USING INDEX"));
    assert!(used_index, "expected explain to mention an index: {}", plan);
}

#[test]
fn empty_filter_matches_all() {
    let db = db();
    let c = db.collection("z");
    c.insert_many(vec![json!({}), json!({}), json!({})])
        .unwrap();
    let all: Vec<Value> = c.find(json!({})).into_vec().unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn drop_collection() {
    let db = db();
    let c = db.collection("trash");
    c.insert_one(json!({ "v": 1 })).unwrap();
    db.drop_collection("trash").unwrap();
    assert!(!db
        .list_collections()
        .unwrap()
        .contains(&"trash".to_string()));
}

#[test]
fn file_persists_across_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.nosqlite");
    {
        let db = Database::open(&path).unwrap();
        db.collection("c").insert_one(json!({ "v": 42 })).unwrap();
    }
    let db = Database::open(&path).unwrap();
    let d = db.collection("c").find_one(json!({})).unwrap().unwrap();
    assert_eq!(d["v"], 42);
}

#[test]
fn sql_injection_in_filter_is_safe() {
    // A field name containing SQL metacharacters must be parameterized.
    // (We don't allow such field names normally, but values must be safe.)
    let db = db();
    let c = db.collection("s");
    c.insert_one(json!({ "name": "Alice'); DROP TABLE s;--" }))
        .unwrap();
    let n = c
        .count(json!({ "name": "Alice'); DROP TABLE s;--" }))
        .unwrap();
    assert_eq!(n, 1);
    // Table should still exist.
    assert_eq!(c.count_all().unwrap(), 1);
}

#[test]
fn type_and_size_operators() {
    let db = db();
    let c = db.collection("t");
    c.insert_many(vec![
        json!({ "v": 1 }),
        json!({ "v": "x" }),
        json!({ "v": [1, 2, 3] }),
        json!({ "v": null }),
    ])
    .unwrap();
    assert_eq!(c.count(json!({ "v": { "$type": "integer" } })).unwrap(), 1);
    assert_eq!(c.count(json!({ "v": { "$type": "text" } })).unwrap(), 1);
    assert_eq!(c.count(json!({ "v": { "$type": "array" } })).unwrap(), 1);
    assert_eq!(c.count(json!({ "v": { "$size": 3 } })).unwrap(), 1);
}
