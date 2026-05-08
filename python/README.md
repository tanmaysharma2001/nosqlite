# nosqlite — Python bindings

MongoDB-style document database for Python, backed by a single SQLite file
via the [nosqlite Rust crate](../README.md).

```python
import nosqlite

db = nosqlite.Database("app.nosqlite")     # or Database() for in-memory
users = db.collection("users")

users.insert_many([
    {"name": "Alice", "age": 30},
    {"name": "Bob",   "age": 22},
])

users.create_index({"age": 1})

for u in users.find({"age": {"$gt": 25}}, sort={"age": -1}, limit=10):
    print(u)

with db.transaction() as tx:
    tx.collection("a").insert_one({"v": 1})
    tx.collection("b").insert_one({"v": 2})
```

## Pydantic ODM

For users who want typed models, the `nosqlite.orm` module wraps a
Collection with a Pydantic-aware Document handle:

```python
from pydantic import BaseModel, Field
from nosqlite import Database
from nosqlite.orm import Document

class User(BaseModel):
    id: str | None = Field(default=None, alias="_id")
    name: str = Field(min_length=1)
    age: int = Field(ge=0)
    model_config = {"populate_by_name": True}

db = Database()
users = Document(db, "users", User)

alice = users.insert(User(name="Alice", age=30))
adults = users.find({"age": {"$gt": 25}}, sort={"age": -1})
```

The Pydantic schema is automatically translated to a JSON-Schema validator
stored on the collection, so writes that bypass the model (raw
`coll.insert_one(...)`) are still rejected.

## Building from source

```sh
pip install maturin
cd python
maturin develop --release
```

The Rust file format is identical between the Python and Rust SDKs — the
same `.nosqlite` file can be opened by either.
