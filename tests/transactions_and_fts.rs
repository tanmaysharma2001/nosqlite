use nosqlite::Database;
use serde_json::json;

#[test]
fn transaction_commits_on_success() {
    let db = Database::open_in_memory().unwrap();
    db.transaction(|tx| {
        tx.collection("a").insert_one(json!({ "v": 1 }))?;
        tx.collection("b").insert_one(json!({ "v": 2 }))?;
        Ok::<_, nosqlite::Error>(())
    })
    .unwrap();
    assert_eq!(db.collection("a").count_all().unwrap(), 1);
    assert_eq!(db.collection("b").count_all().unwrap(), 1);
}

#[test]
fn transaction_rolls_back_on_error() {
    let db = Database::open_in_memory().unwrap();
    db.collection("a").insert_one(json!({ "v": 0 })).unwrap();
    let result: nosqlite::Result<()> = db.transaction(|tx| {
        tx.collection("a").insert_one(json!({ "v": 1 }))?;
        tx.collection("a").insert_one(json!({ "v": 2 }))?;
        Err(nosqlite::Error::InvalidQuery("rollback".into()))
    });
    assert!(result.is_err());
    assert_eq!(db.collection("a").count_all().unwrap(), 1);
}

#[test]
fn transaction_atomic_transfer() {
    let db = Database::open_in_memory().unwrap();
    let accts = db.collection("accts");
    accts
        .insert_many(vec![
            json!({ "_id": "alice", "bal": 100 }),
            json!({ "_id": "bob",   "bal":   0 }),
        ])
        .unwrap();

    db.transaction(|tx| {
        let a = tx.collection("accts");
        a.update_one(json!({ "_id": "alice" }), json!({ "$inc": { "bal": -25 } }))?;
        a.update_one(json!({ "_id": "bob" }), json!({ "$inc": { "bal":  25 } }))?;
        Ok::<_, nosqlite::Error>(())
    })
    .unwrap();

    let alice = accts.find_one(json!({ "_id": "alice" })).unwrap().unwrap();
    let bob = accts.find_one(json!({ "_id": "bob" })).unwrap().unwrap();
    assert_eq!(alice["bal"], 75);
    assert_eq!(bob["bal"], 25);
}

#[test]
fn transaction_find_one_and_count() {
    let db = Database::open_in_memory().unwrap();
    db.collection("c")
        .insert_many(vec![
            json!({ "n": 1 }),
            json!({ "n": 2 }),
            json!({ "n": 3 }),
        ])
        .unwrap();
    db.transaction(|tx| {
        let c = tx.collection("c");
        assert_eq!(c.count_all()?, 3);
        let two = c.find_one(json!({ "n": 2 }))?.unwrap();
        assert_eq!(two["n"], 2);
        Ok::<_, nosqlite::Error>(())
    })
    .unwrap();
}

#[test]
fn fts_text_search_basic() {
    let db = Database::open_in_memory().unwrap();
    let docs = db.collection("docs");
    docs.insert_many(vec![
        json!({ "_id": "1", "title": "The quick brown fox", "body": "jumps over the lazy dog" }),
        json!({ "_id": "2", "title": "Foxes are clever",   "body": "they hunt at night" }),
        json!({ "_id": "3", "title": "Hello world",        "body": "goodbye sky" }),
    ])
    .unwrap();
    docs.create_text_index(&["title", "body"]).unwrap();

    let r = docs
        .find(json!({ "$text": { "$search": "fox" } }))
        .into_vec()
        .unwrap();
    let ids: Vec<&str> = r.iter().map(|d| d["_id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"1"));
    assert!(ids.contains(&"2"));
}

#[test]
fn fts_combined_with_regular_filter() {
    let db = Database::open_in_memory().unwrap();
    let posts = db.collection("posts");
    posts
        .insert_many(vec![
            json!({ "title": "rust crab", "lang": "en", "draft": false }),
            json!({ "title": "rust ferris", "lang": "en", "draft": true }),
            json!({ "title": "Rost krabbe", "lang": "de", "draft": false }),
        ])
        .unwrap();
    posts.create_text_index(&["title"]).unwrap();

    let n = posts
        .count(json!({ "$text": { "$search": "rust" }, "draft": false }))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn fts_index_picks_up_existing_rows_on_create() {
    let db = Database::open_in_memory().unwrap();
    let c = db.collection("c");
    c.insert_many(vec![
        json!({ "title": "alpha bravo" }),
        json!({ "title": "charlie delta" }),
    ])
    .unwrap();
    c.create_text_index(&["title"]).unwrap();

    let r = c
        .find(json!({ "$text": { "$search": "bravo" } }))
        .into_vec()
        .unwrap();
    assert_eq!(r.len(), 1);
}

#[test]
fn fts_reindex_on_update() {
    let db = Database::open_in_memory().unwrap();
    let c = db.collection("c");
    c.create_text_index(&["title"]).unwrap();
    let id = c.insert_one(json!({ "title": "foo" })).unwrap();

    c.update_one(json!({ "_id": &id }), json!({ "$set": { "title": "bar" } }))
        .unwrap();

    let foo = c.count(json!({ "$text": { "$search": "foo" } })).unwrap();
    let bar = c.count(json!({ "$text": { "$search": "bar" } })).unwrap();
    assert_eq!(foo, 0);
    assert_eq!(bar, 1);
}

#[test]
fn fts_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fts.nosqlite");
    {
        let db = Database::open(&path).unwrap();
        let c = db.collection("c");
        c.create_text_index(&["body"]).unwrap();
        c.insert_one(json!({ "body": "the quick brown fox" }))
            .unwrap();
    }
    let db = Database::open(&path).unwrap();
    let n = db
        .collection("c")
        .count(json!({ "$text": { "$search": "brown" } }))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn list_collections_excludes_fts_internals() {
    let db = Database::open_in_memory().unwrap();
    db.collection("posts")
        .create_text_index(&["title"])
        .unwrap();
    db.collection("posts")
        .insert_one(json!({ "title": "x" }))
        .unwrap();
    let names = db.list_collections().unwrap();
    assert_eq!(names, vec!["posts".to_string()]);
}
