"""End-to-end smoke test for the Python bindings."""
import os
import tempfile

import nosqlite


def test_insert_find_count_in_memory():
    db = nosqlite.Database()
    users = db.collection("users")
    users.insert_many([
        {"name": "Alice", "age": 30, "tags": ["admin"]},
        {"name": "Bob",   "age": 22, "tags": ["editor"]},
        {"name": "Carol", "age": 41, "tags": ["editor", "admin"]},
    ])
    assert users.count() == 3
    adults = users.find({"age": {"$gte": 25}}, sort={"age": -1})
    assert [u["name"] for u in adults] == ["Carol", "Alice"]


def test_dotted_path_and_projection():
    db = nosqlite.Database()
    c = db.collection("c")
    c.insert_one({"addr": {"city": "NYC", "zip": "10001"}, "name": "A"})
    c.insert_one({"addr": {"city": "SF"}, "name": "B"})
    nyc = c.find_one({"addr.city": "NYC"}, projection={"name": 1, "_id": 0})
    assert nyc == {"name": "A"}


def test_update_operators():
    db = nosqlite.Database()
    c = db.collection("c")
    _id = c.insert_one({"n": 1, "stale": True})
    c.update_one({"_id": _id}, {"$set": {"n": 5}, "$unset": {"stale": ""}})
    c.update_one({"_id": _id}, {"$inc": {"n": 3}})
    c.update_one({"_id": _id}, {"$push": {"tags": "ok"}})
    c.update_one({"_id": _id}, {"$push": {"tags": {"$each": ["x", "y"]}}})
    d = c.find_one({"_id": _id})
    assert d["n"] == 8
    assert "stale" not in d
    assert d["tags"] == ["ok", "x", "y"]


def test_index_and_explain():
    db = nosqlite.Database()
    c = db.collection("c")
    for i in range(200):
        c.insert_one({"i": i, "tenant": i % 5})
    c.create_index({"i": 1})
    plan = c.explain({"i": 137})
    assert "USING INDEX" in plan


def test_aggregation():
    db = nosqlite.Database()
    sales = db.collection("sales")
    sales.insert_many([
        {"category": "A", "price": 10},
        {"category": "A", "price": 20},
        {"category": "B", "price": 30},
        {"category": "B", "price": 30},
        {"category": "B", "price": 40},
    ])
    out = sales.aggregate([
        {"$group": {
            "_id": "$category",
            "total": {"$sum": "$price"},
            "n":     {"$sum": 1},
        }},
        {"$sort": {"_id": 1}},
    ])
    assert out == [
        {"_id": "A", "total": 30, "n": 2},
        {"_id": "B", "total": 100, "n": 3},
    ]


def test_text_search():
    db = nosqlite.Database()
    posts = db.collection("posts")
    posts.create_text_index(["title", "body"])
    posts.insert_many([
        {"title": "the quick brown fox", "body": "jumps over the lazy dog"},
        {"title": "foxes are clever",    "body": "they hunt at night"},
        {"title": "hello world",         "body": "first post"},
    ])
    hits = posts.find({"$text": {"$search": "fox"}})
    assert len(hits) == 2


def test_transaction_commit_and_rollback():
    db = nosqlite.Database()
    accts = db.collection("accts")
    accts.insert_many([
        {"_id": "alice", "bal": 100},
        {"_id": "bob",   "bal":   0},
    ])

    # Successful transaction commits.
    with db.transaction() as tx:
        a = tx.collection("accts")
        a.update_one({"_id": "alice"}, {"$inc": {"bal": -25}})
        a.update_one({"_id": "bob"},   {"$inc": {"bal":  25}})
    assert accts.find_one({"_id": "alice"})["bal"] == 75
    assert accts.find_one({"_id": "bob"})["bal"]   == 25

    # Exception inside `with` rolls back.
    try:
        with db.transaction() as tx:
            tx.collection("accts").update_one(
                {"_id": "alice"}, {"$inc": {"bal": -1000}})
            raise RuntimeError("nope")
    except RuntimeError:
        pass
    assert accts.find_one({"_id": "alice"})["bal"] == 75


def test_validator():
    db = nosqlite.Database()
    db.set_validator("users", {
        "type": "object",
        "required": ["name", "age"],
        "properties": {
            "name": {"type": "string", "minLength": 1},
            "age":  {"type": "integer", "minimum": 0},
        },
    })
    users = db.collection("users")
    users.insert_one({"name": "Alice", "age": 30})
    failed = False
    try:
        users.insert_one({"name": ""})
    except RuntimeError:
        failed = True
    assert failed
    assert users.count() == 1


def test_file_persists_across_open():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "test.nosqlite")
        db1 = nosqlite.Database(path)
        db1.collection("c").insert_one({"v": 42})
        del db1
        db2 = nosqlite.Database(path)
        d = db2.collection("c").find_one({})
        assert d["v"] == 42


def test_import_export_roundtrip():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "dump.jsonl")
        db_a = nosqlite.Database()
        a = db_a.collection("c")
        a.insert_many([{"n": 1}, {"n": 2}, {"n": 3}])
        a.export_file(path, format="jsonl")
        db_b = nosqlite.Database()
        n = db_b.collection("c").import_file(path, format="jsonl")
        assert n == 3
        assert db_b.collection("c").count() == 3


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
