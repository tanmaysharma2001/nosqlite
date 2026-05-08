"""Tests for the Pydantic ODM wrapper."""
import sys

try:
    from pydantic import BaseModel, Field
except ImportError:
    print("  skip orm tests: pydantic not installed")
    sys.exit(0)

import nosqlite
from nosqlite.orm import Document


class User(BaseModel):
    id: str | None = Field(default=None, alias="_id")
    name: str = Field(min_length=1)
    age: int = Field(ge=0)

    model_config = {"populate_by_name": True}


class Post(BaseModel):
    id: str | None = Field(default=None, alias="_id")
    title: str
    body: str
    tags: list[str] = []

    model_config = {"populate_by_name": True}


def test_insert_and_round_trip():
    db = nosqlite.Database()
    users = Document(db, "users", User)
    alice = users.insert(User(name="Alice", age=30))
    assert alice.id is not None
    assert alice.name == "Alice"

    fetched = users.get(alice.id)
    assert fetched is not None
    assert fetched.name == "Alice"
    assert fetched.age == 30


def test_validator_rejects_bad_data_via_db_layer():
    db = nosqlite.Database()
    users = Document(db, "users", User)
    # Insert via the raw collection bypasses Pydantic but should still hit
    # the JSON-Schema validator we registered.
    failed = False
    try:
        users.collection.insert_one({"name": "", "age": 30})
    except RuntimeError:
        failed = True
    assert failed, "expected validator to reject empty name"


def test_find_with_sort_limit_skip():
    db = nosqlite.Database()
    users = Document(db, "users", User)
    users.insert_many([
        User(name="A", age=10),
        User(name="B", age=20),
        User(name="C", age=30),
        User(name="D", age=40),
    ])
    top2 = users.find({}, sort={"age": -1}, limit=2)
    assert [u.name for u in top2] == ["D", "C"]
    skipped = users.find({}, sort={"age": 1}, skip=1, limit=1)
    assert [u.name for u in skipped] == ["B"]


def test_update_and_replace():
    db = nosqlite.Database()
    users = Document(db, "users", User)
    alice = users.insert(User(name="Alice", age=30))
    n = users.update_one({"_id": alice.id}, {"$inc": {"age": 1}})
    assert n == 1
    after = users.get(alice.id)
    assert after.age == 31

    users.replace(alice.id, User(id=alice.id, name="Alice II", age=99))
    after2 = users.get(alice.id)
    assert after2.name == "Alice II"
    assert after2.age == 99


def test_aggregation_through_orm():
    db = nosqlite.Database()
    sales = Document(db, "sales", Post)
    sales.insert_many([
        Post(title="A", body=""),
        Post(title="A", body=""),
        Post(title="B", body=""),
    ])
    out = sales.aggregate([
        {"$group": {"_id": "$title", "n": {"$sum": 1}}},
        {"$sort": {"_id": 1}},
    ])
    assert out == [{"_id": "A", "n": 2}, {"_id": "B", "n": 1}]


def test_iteration():
    db = nosqlite.Database()
    users = Document(db, "users", User)
    users.insert_many([User(name="A", age=1), User(name="B", age=2)])
    seen = sorted(u.name for u in users)
    assert seen == ["A", "B"]


def test_delete():
    db = nosqlite.Database()
    users = Document(db, "users", User)
    users.insert_many([User(name="A", age=1), User(name="B", age=2)])
    n = users.delete_many({"name": "A"})
    assert n == 1
    assert users.count() == 1


if __name__ == "__main__":
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
