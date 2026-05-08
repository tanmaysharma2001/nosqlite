use nosqlite::Database;
use serde_json::json;

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

#[test]
fn aggregate_match_project_sort() {
    let db = db();
    let c = db.collection("o");
    c.insert_many(vec![
        json!({ "n": 1, "status": "ok",  "amt": 10 }),
        json!({ "n": 2, "status": "bad", "amt": 50 }),
        json!({ "n": 3, "status": "ok",  "amt": 30 }),
        json!({ "n": 4, "status": "ok",  "amt": 20 }),
    ])
    .unwrap();

    let r = c
        .aggregate(vec![
            json!({ "$match": { "status": "ok" } }),
            json!({ "$sort": { "amt": -1 } }),
            json!({ "$project": { "n": 1, "amt": 1, "_id": 0 } }),
        ])
        .unwrap();
    let ns: Vec<i64> = r.iter().map(|d| d["n"].as_i64().unwrap()).collect();
    assert_eq!(ns, vec![3, 4, 1]);
    assert!(r[0].get("_id").is_none());
    assert!(r[0].get("status").is_none());
}

#[test]
fn aggregate_group_sum_avg_count() {
    let db = db();
    let c = db.collection("sales");
    c.insert_many(vec![
        json!({ "category": "A", "price": 10 }),
        json!({ "category": "A", "price": 20 }),
        json!({ "category": "B", "price": 30 }),
        json!({ "category": "B", "price": 30 }),
        json!({ "category": "B", "price": 40 }),
    ])
    .unwrap();

    let r = c
        .aggregate(vec![
            json!({ "$group": {
                "_id": "$category",
                "total": { "$sum": "$price" },
                "avg":   { "$avg": "$price" },
                "n":     { "$sum": 1 },
            }}),
            json!({ "$sort": { "_id": 1 } }),
        ])
        .unwrap();

    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["_id"], "A");
    assert_eq!(r[0]["total"], 30);
    assert_eq!(r[0]["n"], 2);
    assert_eq!(r[1]["_id"], "B");
    assert_eq!(r[1]["total"], 100);
    assert_eq!(r[1]["avg"].as_f64().unwrap(), 100.0 / 3.0);
}

#[test]
fn aggregate_unwind() {
    let db = db();
    let c = db.collection("u");
    c.insert_many(vec![
        json!({ "_id": "a", "tags": ["red", "blue"] }),
        json!({ "_id": "b", "tags": ["green"] }),
        json!({ "_id": "c", "tags": [] }),
    ])
    .unwrap();

    let r = c
        .aggregate(vec![
            json!({ "$unwind": "$tags" }),
            json!({ "$sort": { "_id": 1, "tags": 1 } }),
        ])
        .unwrap();

    assert_eq!(r.len(), 3);
    assert_eq!(r[0]["tags"], "blue");
    assert_eq!(r[1]["tags"], "red");
    assert_eq!(r[2]["tags"], "green");
}

#[test]
fn aggregate_lookup() {
    let db = db();
    let users = db.collection("users");
    let orders = db.collection("orders");
    users
        .insert_many(vec![
            json!({ "_id": "u1", "name": "Alice" }),
            json!({ "_id": "u2", "name": "Bob" }),
        ])
        .unwrap();
    orders
        .insert_many(vec![
            json!({ "user": "u1", "total": 10 }),
            json!({ "user": "u1", "total": 20 }),
            json!({ "user": "u2", "total": 30 }),
        ])
        .unwrap();

    let r = users
        .aggregate(vec![
            json!({ "$lookup": {
                "from": "orders",
                "localField": "_id",
                "foreignField": "user",
                "as": "orders",
            }}),
            json!({ "$sort": { "_id": 1 } }),
        ])
        .unwrap();

    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["orders"].as_array().unwrap().len(), 2);
    assert_eq!(r[1]["orders"].as_array().unwrap().len(), 1);
}

#[test]
fn aggregate_count_stage() {
    let db = db();
    let c = db.collection("c");
    c.insert_many(vec![
        json!({ "n": 1 }),
        json!({ "n": 2 }),
        json!({ "n": 3 }),
        json!({ "n": 4 }),
    ])
    .unwrap();
    let r = c
        .aggregate(vec![
            json!({ "$match": { "n": { "$gt": 2 } } }),
            json!({ "$count": "matched" }),
        ])
        .unwrap();
    assert_eq!(r, vec![json!({ "matched": 2 })]);
}

#[test]
fn aggregate_addfields_with_expr() {
    let db = db();
    let c = db.collection("e");
    c.insert_many(vec![
        json!({ "first": "Alice", "last": "Smith" }),
        json!({ "first": "Bob", "last": "Jones" }),
    ])
    .unwrap();
    let r = c
        .aggregate(vec![json!({
            "$addFields": {
                "full": { "$concat": ["$first", " ", "$last"] },
                "shouty": { "$toUpper": "$first" },
            }
        })])
        .unwrap();
    let r: Vec<_> = r
        .iter()
        .map(|d| {
            (
                d["full"].as_str().unwrap().to_string(),
                d["shouty"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(r.contains(&("Alice Smith".to_string(), "ALICE".to_string())));
    assert!(r.contains(&("Bob Jones".to_string(), "BOB".to_string())));
}
