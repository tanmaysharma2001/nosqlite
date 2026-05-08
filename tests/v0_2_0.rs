//! Tests for the v0.2.0 surface: upsert, find_one_and_*, distinct, bulk_write,
//! and `$expr` filters.

use nosqlite::{
    BulkWriteOptions, Database, FindOneAndUpdateOptions, ReturnDocument, UpdateOptions, WriteOp,
};
use serde_json::{json, Value};

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

#[test]
fn upsert_inserts_when_no_match() {
    let db = db();
    let c = db.collection("users");
    let r = c
        .update_one_with_options(
            json!({ "name": "Alice" }),
            json!({ "$set": { "age": 30 } }),
            UpdateOptions { upsert: true },
        )
        .unwrap();
    assert_eq!(r.matched_count, 0);
    assert_eq!(r.modified_count, 0);
    let id = r.upserted_id.expect("expected upserted_id");
    let d = c.find_one(json!({ "_id": &id })).unwrap().unwrap();
    assert_eq!(d["name"], "Alice");
    assert_eq!(d["age"], 30);
}

#[test]
fn upsert_does_not_insert_when_match_exists() {
    let db = db();
    let c = db.collection("u");
    c.insert_one(json!({ "_id": "x", "name": "A", "n": 1 }))
        .unwrap();
    let r = c
        .update_one_with_options(
            json!({ "_id": "x" }),
            json!({ "$inc": { "n": 1 } }),
            UpdateOptions { upsert: true },
        )
        .unwrap();
    assert_eq!(r.matched_count, 1);
    assert!(r.upserted_id.is_none());
    let d = c.find_one(json!({ "_id": "x" })).unwrap().unwrap();
    assert_eq!(d["n"], 2);
    assert_eq!(c.count_all().unwrap(), 1);
}

#[test]
fn upsert_skips_operator_filter_clauses() {
    // Filter `{age: {$gt: 18}}` should NOT seed `age` into the new doc.
    let db = db();
    let c = db.collection("u");
    let r = c
        .update_one_with_options(
            json!({ "name": "B", "age": { "$gt": 18 } }),
            json!({ "$set": { "active": true } }),
            UpdateOptions { upsert: true },
        )
        .unwrap();
    let id = r.upserted_id.unwrap();
    let d = c.find_one(json!({ "_id": &id })).unwrap().unwrap();
    assert_eq!(d["name"], "B");
    assert!(
        d.get("age").is_none(),
        "age should not be seeded from $gt clause"
    );
    assert_eq!(d["active"], true);
}

#[test]
fn upsert_replacement_style() {
    let db = db();
    let c = db.collection("u");
    let r = c
        .replace_one_with_options(
            json!({ "_id": "fixed" }),
            json!({ "name": "Replaced", "n": 1 }),
            UpdateOptions { upsert: true },
        )
        .unwrap();
    assert_eq!(r.upserted_id.as_deref(), Some("fixed"));
    let d = c.find_one(json!({ "_id": "fixed" })).unwrap().unwrap();
    assert_eq!(d["name"], "Replaced");
    assert_eq!(d["n"], 1);
}

#[test]
fn find_one_and_update_returns_before_by_default() {
    let db = db();
    let c = db.collection("u");
    c.insert_one(json!({ "_id": "x", "n": 1 })).unwrap();
    let r = c
        .find_one_and_update(json!({ "_id": "x" }), json!({ "$inc": { "n": 1 } }))
        .unwrap()
        .unwrap();
    assert_eq!(r["n"], 1, "default Before should return pre-update value");
    let d = c.find_one(json!({ "_id": "x" })).unwrap().unwrap();
    assert_eq!(d["n"], 2, "underlying doc should be updated");
}

#[test]
fn find_one_and_update_after_returns_new() {
    let db = db();
    let c = db.collection("u");
    c.insert_one(json!({ "_id": "x", "n": 1 })).unwrap();
    let r = c
        .find_one_and_update_with_options(
            json!({ "_id": "x" }),
            json!({ "$inc": { "n": 1 } }),
            FindOneAndUpdateOptions {
                return_document: ReturnDocument::After,
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(r["n"], 2);
}

#[test]
fn find_one_and_update_no_match_returns_none() {
    let db = db();
    let c = db.collection("u");
    let r = c
        .find_one_and_update(json!({ "missing": true }), json!({ "$set": { "x": 1 } }))
        .unwrap();
    assert!(r.is_none());
}

#[test]
fn find_one_and_update_upsert_after_returns_inserted() {
    let db = db();
    let c = db.collection("u");
    let r = c
        .find_one_and_update_with_options(
            json!({ "name": "Z" }),
            json!({ "$set": { "n": 5 } }),
            FindOneAndUpdateOptions {
                upsert: true,
                return_document: ReturnDocument::After,
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(r["name"], "Z");
    assert_eq!(r["n"], 5);
    assert!(r.get("_id").is_some());
}

#[test]
fn find_one_and_replace_swaps_doc() {
    let db = db();
    let c = db.collection("u");
    c.insert_one(json!({ "_id": "k", "old": true })).unwrap();
    let _ = c
        .find_one_and_replace(json!({ "_id": "k" }), json!({ "fresh": true }))
        .unwrap()
        .unwrap();
    let d = c.find_one(json!({ "_id": "k" })).unwrap().unwrap();
    assert!(d.get("old").is_none());
    assert_eq!(d["fresh"], true);
    assert_eq!(d["_id"], "k");
}

#[test]
fn find_one_and_delete_removes_and_returns_doc() {
    let db = db();
    let c = db.collection("u");
    c.insert_one(json!({ "_id": "gone", "v": 7 })).unwrap();
    let r = c
        .find_one_and_delete(json!({ "_id": "gone" }))
        .unwrap()
        .unwrap();
    assert_eq!(r["v"], 7);
    assert_eq!(c.count_all().unwrap(), 0);
}

#[test]
fn find_one_and_delete_no_match_returns_none() {
    let db = db();
    let c = db.collection("u");
    let r = c.find_one_and_delete(json!({ "_id": "nope" })).unwrap();
    assert!(r.is_none());
}

#[test]
fn distinct_returns_unique_scalars() {
    let db = db();
    let c = db.collection("p");
    for color in ["red", "blue", "red", "green", "blue", "red"] {
        c.insert_one(json!({ "color": color })).unwrap();
    }
    let mut got = c.distinct("color", json!({})).unwrap();
    got.sort_by_key(|v| v.as_str().unwrap_or("").to_string());
    assert_eq!(got, vec![json!("blue"), json!("green"), json!("red")]);
}

#[test]
fn distinct_with_filter() {
    let db = db();
    let c = db.collection("p");
    c.insert_many(vec![
        json!({ "color": "red", "n": 1 }),
        json!({ "color": "blue", "n": 1 }),
        json!({ "color": "red", "n": 2 }),
    ])
    .unwrap();
    let got = c.distinct("color", json!({ "n": 1 })).unwrap();
    let mut s: Vec<&str> = got.iter().filter_map(|v| v.as_str()).collect();
    s.sort();
    assert_eq!(s, vec!["blue", "red"]);
}

#[test]
fn distinct_unrolls_arrays() {
    let db = db();
    let c = db.collection("p");
    c.insert_one(json!({ "tags": ["a", "b"] })).unwrap();
    c.insert_one(json!({ "tags": ["b", "c"] })).unwrap();
    let raw = c.distinct("tags", json!({})).unwrap();
    let mut got: Vec<String> = raw
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    got.sort();
    assert_eq!(got, vec!["a", "b", "c"]);
}

#[test]
fn bulk_write_mixed_ops() {
    let db = db();
    let c = db.collection("b");
    c.insert_one(json!({ "_id": "a", "n": 1 })).unwrap();
    let r = c
        .bulk_write(vec![
            WriteOp::InsertOne {
                document: json!({ "_id": "b", "n": 2 }),
            },
            WriteOp::UpdateOne {
                filter: json!({ "_id": "a" }),
                update: json!({ "$inc": { "n": 10 } }),
                upsert: false,
            },
            WriteOp::UpdateOne {
                filter: json!({ "_id": "c" }),
                update: json!({ "$set": { "n": 99 } }),
                upsert: true,
            },
            WriteOp::DeleteOne {
                filter: json!({ "_id": "b" }),
            },
        ])
        .unwrap();
    assert_eq!(r.inserted_count, 1);
    assert_eq!(r.matched_count, 1);
    assert_eq!(r.modified_count, 1);
    assert_eq!(r.deleted_count, 1);
    assert_eq!(r.upserted_ids.len(), 1);
    assert_eq!(r.upserted_ids[0].0, 2);
    assert_eq!(r.upserted_ids[0].1, "c");

    assert_eq!(c.find_one(json!({ "_id": "a" })).unwrap().unwrap()["n"], 11);
    assert!(c.find_one(json!({ "_id": "b" })).unwrap().is_none());
    assert_eq!(c.find_one(json!({ "_id": "c" })).unwrap().unwrap()["n"], 99);
}

#[test]
fn bulk_write_ordered_failure_rolls_back() {
    let db = db();
    let c = db.collection("b");
    c.insert_one(json!({ "_id": "x", "n": 1 })).unwrap();
    // Second op duplicates the _id, which violates the PRIMARY KEY constraint.
    let res = c.bulk_write(vec![
        WriteOp::UpdateOne {
            filter: json!({ "_id": "x" }),
            update: json!({ "$inc": { "n": 1 } }),
            upsert: false,
        },
        WriteOp::InsertOne {
            document: json!({ "_id": "x", "dup": true }),
        },
    ]);
    assert!(res.is_err(), "expected ordered bulk to fail");
    let d = c.find_one(json!({ "_id": "x" })).unwrap().unwrap();
    assert_eq!(d["n"], 1, "first op should have rolled back");
    assert_eq!(c.count_all().unwrap(), 1);
}

#[test]
fn bulk_write_unordered_continues_past_failure() {
    let db = db();
    let c = db.collection("b");
    c.insert_one(json!({ "_id": "x", "n": 1 })).unwrap();
    let r = c
        .bulk_write_with_options(
            vec![
                WriteOp::InsertOne {
                    document: json!({ "_id": "x", "dup": true }),
                },
                WriteOp::InsertOne {
                    document: json!({ "_id": "y", "n": 2 }),
                },
            ],
            BulkWriteOptions { ordered: false },
        )
        .unwrap();
    // First insert fails, second succeeds.
    assert_eq!(r.inserted_count, 1);
    assert!(c.find_one(json!({ "_id": "y" })).unwrap().is_some());
}

#[test]
fn expr_compares_two_fields() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![
        json!({ "a": 5, "b": 3 }),
        json!({ "a": 2, "b": 4 }),
        json!({ "a": 7, "b": 7 }),
    ])
    .unwrap();
    let n = c
        .count(json!({ "$expr": { "$gt": ["$a", "$b"] } }))
        .unwrap();
    assert_eq!(n, 1);
    let n = c
        .count(json!({ "$expr": { "$eq": ["$a", "$b"] } }))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn expr_combines_with_other_filter_clauses() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![
        json!({ "kind": "A", "a": 5, "b": 1 }),
        json!({ "kind": "A", "a": 2, "b": 4 }),
        json!({ "kind": "B", "a": 9, "b": 1 }),
    ])
    .unwrap();
    let n = c
        .count(json!({
            "kind": "A",
            "$expr": { "$gt": ["$a", "$b"] }
        }))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn expr_in_find_with_limit() {
    let db = db();
    let c = db.collection("e");
    for i in 0..10 {
        c.insert_one(json!({ "a": i, "b": 5 })).unwrap();
    }
    // Should find a > b: i in 6..=9 -> 4 docs; with limit 2 -> 2 docs.
    let docs: Vec<Value> = c
        .find(json!({ "$expr": { "$gt": ["$a", "$b"] } }))
        .limit(2)
        .into_vec()
        .unwrap();
    assert_eq!(docs.len(), 2);
    for d in docs {
        assert!(d["a"].as_i64().unwrap() > 5);
    }
}

#[test]
fn expr_with_cond() {
    let db = db();
    let c = db.collection("e");
    c.insert_one(json!({ "v": 1 })).unwrap();
    c.insert_one(json!({ "v": 0 })).unwrap();
    let n = c
        .count(json!({
            "$expr": { "$cond": [ "$v", true, false ] }
        }))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn expr_in_update_filter() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![
        json!({ "_id": "x", "a": 5, "b": 1 }),
        json!({ "_id": "y", "a": 1, "b": 5 }),
    ])
    .unwrap();
    let n = c
        .update_many(
            json!({ "$expr": { "$gt": ["$a", "$b"] } }),
            json!({ "$set": { "winner": true } }),
        )
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        c.find_one(json!({ "_id": "x" })).unwrap().unwrap()["winner"],
        true
    );
    assert!(c
        .find_one(json!({ "_id": "y" }))
        .unwrap()
        .unwrap()
        .get("winner")
        .is_none());
}

#[test]
fn expr_in_delete_filter() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![
        json!({ "_id": "x", "a": 1, "b": 5 }),
        json!({ "_id": "y", "a": 5, "b": 1 }),
    ])
    .unwrap();
    let n = c
        .delete_many(json!({ "$expr": { "$gt": ["$a", "$b"] } }))
        .unwrap();
    assert_eq!(n, 1);
    assert!(c.find_one(json!({ "_id": "y" })).unwrap().is_none());
    assert!(c.find_one(json!({ "_id": "x" })).unwrap().is_some());
}

#[test]
fn bulk_write_in_transaction() {
    let db = db();
    db.collection("b")
        .insert_one(json!({ "_id": "a", "n": 1 }))
        .unwrap();
    db.transaction(|tx| {
        let c = tx.collection("b");
        c.bulk_write(vec![
            WriteOp::InsertOne {
                document: json!({ "_id": "b", "n": 2 }),
            },
            WriteOp::UpdateOne {
                filter: json!({ "_id": "a" }),
                update: json!({ "$inc": { "n": 10 } }),
                upsert: false,
            },
        ])?;
        Ok(())
    })
    .unwrap();
    let c = db.collection("b");
    assert_eq!(c.find_one(json!({ "_id": "a" })).unwrap().unwrap()["n"], 11);
    assert_eq!(c.find_one(json!({ "_id": "b" })).unwrap().unwrap()["n"], 2);
}

#[test]
fn expr_nested_in_or() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![
        json!({ "_id": "x", "kind": "A", "a": 1, "b": 5 }),
        json!({ "_id": "y", "kind": "B", "a": 5, "b": 1 }),
        json!({ "_id": "z", "kind": "C", "a": 0, "b": 0 }),
    ])
    .unwrap();
    // kind=="A" OR a > b. Matches x (kind A) and y (5 > 1).
    let docs = c
        .find(json!({
            "$or": [
                { "kind": "A" },
                { "$expr": { "$gt": ["$a", "$b"] } }
            ]
        }))
        .into_vec()
        .unwrap();
    let mut ids: Vec<String> = docs
        .iter()
        .map(|d| d["_id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["x", "y"]);
}

#[test]
fn expr_nested_in_and() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![
        json!({ "_id": "x", "kind": "A", "a": 5, "b": 1 }),
        json!({ "_id": "y", "kind": "A", "a": 2, "b": 4 }),
        json!({ "_id": "z", "kind": "B", "a": 9, "b": 1 }),
    ])
    .unwrap();
    // kind=="A" AND a > b. Only x matches.
    let docs = c
        .find(json!({
            "$and": [
                { "kind": "A" },
                { "$expr": { "$gt": ["$a", "$b"] } }
            ]
        }))
        .into_vec()
        .unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["_id"], "x");
}

#[test]
fn expr_nested_deep() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![
        json!({ "_id": "x", "kind": "A", "a": 5, "b": 1, "n": 1 }),
        json!({ "_id": "y", "kind": "A", "a": 1, "b": 5, "n": 2 }),
        json!({ "_id": "z", "kind": "B", "a": 7, "b": 1, "n": 1 }),
    ])
    .unwrap();
    // n==1 AND (kind=="A" OR a > b)
    let docs = c
        .find(json!({
            "$and": [
                { "n": 1 },
                { "$or": [
                    { "kind": "A" },
                    { "$expr": { "$gt": ["$a", "$b"] } }
                ]}
            ]
        }))
        .into_vec()
        .unwrap();
    let mut ids: Vec<String> = docs
        .iter()
        .map(|d| d["_id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["x", "z"]);
}

#[test]
fn expr_nested_in_or_with_count_and_delete() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![
        json!({ "_id": "x", "tag": "keep", "a": 1, "b": 9 }),
        json!({ "_id": "y", "tag": "drop", "a": 5, "b": 1 }),
        json!({ "_id": "z", "tag": "drop", "a": 1, "b": 5 }),
    ])
    .unwrap();
    let filter = json!({
        "$or": [
            { "tag": "keep" },
            { "$expr": { "$gt": ["$a", "$b"] } }
        ]
    });
    assert_eq!(c.count(filter.clone()).unwrap(), 2);
    let n = c.delete_many(filter).unwrap();
    assert_eq!(n, 2);
    assert_eq!(c.count_all().unwrap(), 1);
    assert_eq!(c.find_one(json!({})).unwrap().unwrap()["_id"], "z");
}
