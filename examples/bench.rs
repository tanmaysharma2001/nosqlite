//! Microbenchmark for NoSQLite. Run with `--release` to get realistic numbers.
//!
//! ```text
//! cargo run --release --example bench
//! ```
//!
//! Reports throughput / latency for the operations the roadmap calls out:
//! bulk insert, indexed find, full-scan find, indexed update, and delete.
//! Each scenario is run twice — once without an index, once with — so the
//! benefit of indexing is visible in the output.

use nosqlite::Database;
use serde_json::json;
use std::time::Instant;

const N: usize = 100_000;

fn main() -> nosqlite::Result<()> {
    println!("NoSQLite microbenchmark — {} documents", fmt_n(N));
    println!("=====================================");

    let db = Database::open_in_memory()?;
    let users = db.collection("users");

    let docs: Vec<_> = (0..N)
        .map(|i| {
            json!({
                "_id":    format!("u{}", i),
                "name":   format!("user-{}", i),
                "tenant": (i % 100) as i64,
                "score":  (i % 1000) as i64,
                "tags":   vec![format!("t{}", i % 10), "active".to_string()],
                "addr":   { "city": city(i), "zip": format!("{:05}", i % 100_000) },
            })
        })
        .collect();

    // ---- Insert ----------------------------------------------------------
    let t = Instant::now();
    users.insert_many(docs)?;
    let elapsed = t.elapsed();
    report("insert_many (1 batch)", N, elapsed);

    // ---- Full scan: equality on unindexed field --------------------------
    let q = json!({ "tenant": 42 });
    bench("find equality, no index   ", &|| {
        users.count(q.clone()).unwrap()
    });

    // ---- Add an index ----------------------------------------------------
    users.create_index(json!({ "tenant": 1 }))?;
    bench("find equality, with index ", &|| {
        users.count(q.clone()).unwrap()
    });

    // ---- Range query -----------------------------------------------------
    let r = json!({ "score": { "$gte": 500, "$lt": 600 } });
    bench("find range, no index      ", &|| {
        users.count(r.clone()).unwrap()
    });
    users.create_index(json!({ "score": 1 }))?;
    bench("find range, with index    ", &|| {
        users.count(r.clone()).unwrap()
    });

    // ---- Update one ------------------------------------------------------
    let t = Instant::now();
    let mut total = 0u64;
    for i in 0..1_000 {
        total += users.update_one(
            json!({ "_id": format!("u{}", i) }),
            json!({ "$inc": { "score": 1 } }),
        )?;
    }
    report(
        &format!("update_one (x1000, {} matched)", total),
        1_000,
        t.elapsed(),
    );

    // ---- Aggregation -----------------------------------------------------
    let t = Instant::now();
    let _r = users.aggregate(vec![
        json!({ "$match": { "score": { "$gte": 500 } } }),
        json!({ "$group": { "_id": "$tenant", "n": { "$sum": 1 }, "max": { "$max": "$score" } } }),
        json!({ "$sort": { "n": -1 } }),
        json!({ "$limit": 10 }),
    ])?;
    report("aggregate group+sort+limit", 1, t.elapsed());

    // ---- Delete ----------------------------------------------------------
    let t = Instant::now();
    let n = users.delete_many(json!({ "tenant": { "$gte": 90 } }))?;
    report(
        &format!("delete_many (range, {} rows)", n),
        n as usize,
        t.elapsed(),
    );

    // ---- File on-disk size for the post-bench database --------------------
    println!();
    println!("(in-memory database; rerun with Database::open(path) for on-disk size)");
    Ok(())
}

fn city(i: usize) -> &'static str {
    const CITIES: &[&str] = &[
        "NYC", "SF", "Seattle", "Austin", "Boston", "Chicago", "Denver",
    ];
    CITIES[i % CITIES.len()]
}

fn bench(label: &str, mut f: &dyn Fn() -> i64) {
    // Warm up.
    let _ = f();
    let runs = 5;
    let t = Instant::now();
    let mut last = 0i64;
    for _ in 0..runs {
        last = f();
    }
    let elapsed = t.elapsed() / runs as u32;
    println!(
        "  {:30}  {:>10.3} ms   (matched {})",
        label,
        elapsed.as_secs_f64() * 1000.0,
        fmt_n(last as usize)
    );
    let _ = &mut f;
}

fn report(label: &str, count: usize, d: std::time::Duration) {
    let secs = d.as_secs_f64();
    let per_op = if count > 0 {
        d.as_nanos() as f64 / count as f64 / 1000.0
    } else {
        0.0
    };
    let throughput = if secs > 0.0 { count as f64 / secs } else { 0.0 };
    println!(
        "  {:30}  {:>10.3} ms   ({:.1} ops/s, {:.1} µs/op)",
        label,
        secs * 1000.0,
        throughput,
        per_op
    );
}

fn fmt_n(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}
