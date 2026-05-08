use nosqlite::Database;
use serde_json::json;

fn main() -> nosqlite::Result<()> {
    let db = Database::open_in_memory()?;
    let users = db.collection("users");

    users.insert_many(vec![
        json!({ "name": "Alice", "age": 30, "tags": ["admin"] }),
        json!({ "name": "Bob",   "age": 22, "tags": ["editor"] }),
        json!({ "name": "Carol", "age": 41, "tags": ["editor", "admin"] }),
    ])?;

    users.create_index(json!({ "age": 1 }))?;

    let adults = users
        .find(json!({ "age": { "$gte": 25 } }))
        .sort(json!({ "age": -1 }))
        .project(json!({ "name": 1, "age": 1, "_id": 0 }))
        .into_vec()?;

    for u in &adults {
        println!("{}", u);
    }

    users.update_many(
        json!({ "tags": { "$exists": true } }),
        json!({ "$push": { "tags": "active" } }),
    )?;

    let plan = users.find(json!({ "age": { "$gte": 25 } })).explain()?;
    println!("\n{}", plan);

    Ok(())
}
