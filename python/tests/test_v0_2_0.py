"""Smoke tests for the v0.2.0 surface in the Python binding."""
import nosqlite


def test_upsert_inserts_when_no_match():
    db = nosqlite.Database()
    c = db.collection("u")
    r = c.update_one_with_options(
        {"name": "Alice"}, {"$set": {"age": 30}}, upsert=True,
    )
    assert r["matched_count"] == 0
    assert r["modified_count"] == 0
    assert r["upserted_id"]
    d = c.find_one({"_id": r["upserted_id"]})
    assert d["name"] == "Alice"
    assert d["age"] == 30


def test_upsert_existing_returns_no_upserted_id():
    db = nosqlite.Database()
    c = db.collection("u")
    c.insert_one({"_id": "x", "n": 1})
    r = c.update_one_with_options(
        {"_id": "x"}, {"$inc": {"n": 1}}, upsert=True,
    )
    assert r["matched_count"] == 1
    assert r["modified_count"] == 1
    assert r["upserted_id"] is None
    assert c.find_one({"_id": "x"})["n"] == 2


def test_find_one_and_update_default_returns_before():
    db = nosqlite.Database()
    c = db.collection("u")
    c.insert_one({"_id": "x", "n": 1})
    before = c.find_one_and_update({"_id": "x"}, {"$inc": {"n": 1}})
    assert before["n"] == 1
    assert c.find_one({"_id": "x"})["n"] == 2


def test_find_one_and_update_after():
    db = nosqlite.Database()
    c = db.collection("u")
    c.insert_one({"_id": "x", "n": 1})
    after = c.find_one_and_update(
        {"_id": "x"}, {"$inc": {"n": 1}}, return_document="after",
    )
    assert after["n"] == 2


def test_find_one_and_update_upsert_after():
    db = nosqlite.Database()
    c = db.collection("u")
    r = c.find_one_and_update(
        {"name": "Z"}, {"$set": {"n": 5}}, upsert=True, return_document="after",
    )
    assert r["name"] == "Z"
    assert r["n"] == 5


def test_find_one_and_replace_swaps_doc():
    db = nosqlite.Database()
    c = db.collection("u")
    c.insert_one({"_id": "k", "old": True})
    c.find_one_and_replace({"_id": "k"}, {"fresh": True})
    d = c.find_one({"_id": "k"})
    assert "old" not in d
    assert d["fresh"] is True
    assert d["_id"] == "k"


def test_find_one_and_delete_removes_and_returns():
    db = nosqlite.Database()
    c = db.collection("u")
    c.insert_one({"_id": "gone", "v": 7})
    r = c.find_one_and_delete({"_id": "gone"})
    assert r["v"] == 7
    assert c.count() == 0


def test_distinct_returns_unique_values():
    db = nosqlite.Database()
    c = db.collection("p")
    for color in ["red", "blue", "red", "green", "blue"]:
        c.insert_one({"color": color})
    assert sorted(c.distinct("color")) == ["blue", "green", "red"]


def test_distinct_unrolls_arrays():
    db = nosqlite.Database()
    c = db.collection("p")
    c.insert_one({"tags": ["a", "b"]})
    c.insert_one({"tags": ["b", "c"]})
    assert sorted(c.distinct("tags")) == ["a", "b", "c"]


def test_bulk_write_mixed_ops():
    db = nosqlite.Database()
    c = db.collection("b")
    c.insert_one({"_id": "a", "n": 1})
    r = c.bulk_write([
        {"insertOne": {"document": {"_id": "b", "n": 2}}},
        {"updateOne": {"filter": {"_id": "a"}, "update": {"$inc": {"n": 10}}}},
        {"updateOne": {"filter": {"_id": "c"}, "update": {"$set": {"n": 99}}, "upsert": True}},
        {"deleteOne": {"filter": {"_id": "b"}}},
    ])
    assert r["inserted_count"] == 1
    assert r["matched_count"] == 1
    assert r["modified_count"] == 1
    assert r["deleted_count"] == 1
    assert len(r["upserted_ids"]) == 1
    assert r["upserted_ids"][0] == {"index": 2, "_id": "c"}
    assert c.find_one({"_id": "a"})["n"] == 11
    assert c.find_one({"_id": "c"})["n"] == 99


def test_bulk_write_unordered_continues_past_failure():
    db = nosqlite.Database()
    c = db.collection("b")
    c.insert_one({"_id": "x", "n": 1})
    r = c.bulk_write(
        [
            {"insertOne": {"document": {"_id": "x", "dup": True}}},  # fails
            {"insertOne": {"document": {"_id": "y", "n": 2}}},       # succeeds
        ],
        ordered=False,
    )
    assert r["inserted_count"] == 1
    assert c.find_one({"_id": "y"}) is not None


def test_expr_filter():
    db = nosqlite.Database()
    c = db.collection("e")
    c.insert_many([
        {"a": 5, "b": 3},
        {"a": 2, "b": 4},
        {"a": 7, "b": 7},
    ])
    assert c.count({"$expr": {"$gt": ["$a", "$b"]}}) == 1
    assert c.count({"$expr": {"$eq": ["$a", "$b"]}}) == 1


def test_v020_methods_work_inside_transaction():
    db = nosqlite.Database()
    db.collection("b").insert_one({"_id": "a", "n": 1})
    with db.transaction() as tx:
        c = tx.collection("b")
        r = c.bulk_write([
            {"insertOne": {"document": {"_id": "b", "n": 2}}},
            {"updateOne": {"filter": {"_id": "a"}, "update": {"$inc": {"n": 10}}}},
        ])
        assert r["inserted_count"] == 1
        assert r["modified_count"] == 1
    c = db.collection("b")
    assert c.find_one({"_id": "a"})["n"] == 11
    assert c.find_one({"_id": "b"})["n"] == 2


if __name__ == "__main__":
    import sys
    failures = 0
    for name, fn in list(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"  ok   {name}")
            except AssertionError as e:
                failures += 1
                print(f"  FAIL {name}: {e}")
            except Exception as e:
                failures += 1
                print(f"  FAIL {name}: {type(e).__name__}: {e}")
    sys.exit(1 if failures else 0)
