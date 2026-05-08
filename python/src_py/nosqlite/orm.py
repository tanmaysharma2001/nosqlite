"""Pydantic-based ODM for NoSQLite.

Define a Pydantic model, hand it to ``Document(db, "users", User)``, and the
returned wrapper exposes ``insert``, ``find``, ``find_one``, etc., automatically
serialising your model instances on the way in and parsing them on the way
out. Schemas are translated to JSON-Schema validators so bad documents are
rejected at the storage layer.

```python
from pydantic import BaseModel, Field
from nosqlite import Database
from nosqlite.orm import Document

class User(BaseModel):
    id: str | None = Field(default=None, alias="_id")
    name: str = Field(min_length=1)
    age: int = Field(ge=0)

    model_config = {"populate_by_name": True}

db = Database("app.nosqlite")
users = Document(db, "users", User)

alice = users.insert(User(name="Alice", age=30))
adults = users.find({"age": {"$gt": 25}}, sort={"age": -1})
```

Pydantic 2 is required. The wrapper does *not* couple your domain models to
the database — you can still construct, serialise, and validate models
independently.
"""

from __future__ import annotations

from typing import Generic, Iterable, Iterator, List, Optional, Type, TypeVar

try:
    from pydantic import BaseModel
except ImportError as e:  # pragma: no cover - pydantic is optional at install time
    raise ImportError(
        "nosqlite.orm requires pydantic >= 2. Install with `pip install pydantic`."
    ) from e

from . import Database

T = TypeVar("T", bound=BaseModel)


def _model_to_dict(model: BaseModel) -> dict:
    # Pydantic 2: dump by alias so {"id": ...} → {"_id": ...} via Field(alias).
    # Exclude None for the id so SQLite generates one when missing.
    return model.model_dump(by_alias=True, exclude_none=True)


def _dict_to_model(model_cls: Type[T], doc: dict) -> T:
    return model_cls.model_validate(doc)


def _pydantic_json_schema(model_cls: Type[BaseModel]) -> dict:
    """Translate a Pydantic schema into a JSON-Schema document the
    NoSQLite validator can consume."""
    raw = model_cls.model_json_schema(by_alias=True)
    # Pydantic emits $defs / refs; for our minimal validator we strip
    # references to nested models and keep just type / required / properties.
    out = {
        "type": "object",
    }
    if "required" in raw:
        out["required"] = [r for r in raw["required"] if r != "_id"]
    if "properties" in raw:
        props = {}
        for name, schema in raw["properties"].items():
            if name == "_id":
                continue
            props[name] = _simplify(schema)
        out["properties"] = props
    return out


def _simplify(schema: dict) -> dict:
    """Drop refs / nested model definitions, keep concrete keywords."""
    keep = {
        "type",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "enum",
        "const",
    }
    out = {k: v for k, v in schema.items() if k in keep}
    # Pydantic uses "anyOf": [{type: x}, {type: null}] for Optional.
    if "anyOf" in schema:
        types = []
        for v in schema["anyOf"]:
            if "type" in v:
                types.append(v["type"])
        if types:
            out.setdefault("type", types if len(types) > 1 else types[0])
    return out


class Document(Generic[T]):
    """A typed wrapper over a NoSQLite collection bound to a Pydantic model."""

    def __init__(
        self,
        db: Database,
        name: str,
        model: Type[T],
        *,
        validate: bool = True,
        indexes: Optional[Iterable[dict]] = None,
    ):
        self._db = db
        self._name = name
        self._model = model
        self._coll = db.collection(name)
        if validate:
            db.set_validator(name, _pydantic_json_schema(model), "strict")
        for ix in indexes or ():
            keys = ix.get("keys") or {k: v for k, v in ix.items() if k != "unique" and k != "name"}
            self._coll.create_index(keys, unique=ix.get("unique", False), name=ix.get("name"))

    @property
    def collection(self):
        """Drop down to the raw `Collection` for indexes / aggregation / FTS."""
        return self._coll

    @property
    def name(self) -> str:
        return self._name

    def insert(self, obj: T) -> T:
        """Insert a model instance, returning a *new* instance with the id
        populated."""
        new_id = self._coll.insert_one(_model_to_dict(obj))
        # Re-fetch so server-generated fields propagate.
        d = self._coll.find_one({"_id": new_id})
        return _dict_to_model(self._model, d) if d is not None else obj

    def insert_many(self, objs: Iterable[T]) -> List[T]:
        objs = list(objs)
        ids = self._coll.insert_many([_model_to_dict(o) for o in objs])
        out: List[T] = []
        for i in ids:
            d = self._coll.find_one({"_id": i})
            if d is not None:
                out.append(_dict_to_model(self._model, d))
        return out

    def find(
        self,
        filter: Optional[dict] = None,
        *,
        sort: Optional[dict] = None,
        limit: Optional[int] = None,
        skip: Optional[int] = None,
    ) -> List[T]:
        docs = self._coll.find(
            filter or {},
            sort=sort,
            limit=limit,
            skip=skip,
        )
        return [_dict_to_model(self._model, d) for d in docs]

    def find_one(self, filter: Optional[dict] = None) -> Optional[T]:
        d = self._coll.find_one(filter or {})
        return _dict_to_model(self._model, d) if d is not None else None

    def get(self, _id: str) -> Optional[T]:
        return self.find_one({"_id": _id})

    def count(self, filter: Optional[dict] = None) -> int:
        return self._coll.count(filter or {})

    def update_one(self, filter: dict, update: dict) -> int:
        return self._coll.update_one(filter, update)

    def update_many(self, filter: dict, update: dict) -> int:
        return self._coll.update_many(filter, update)

    def replace(self, _id: str, obj: T) -> int:
        return self._coll.replace_one({"_id": _id}, _model_to_dict(obj))

    def delete_one(self, filter: dict) -> int:
        return self._coll.delete_one(filter)

    def delete_many(self, filter: dict) -> int:
        return self._coll.delete_many(filter)

    def aggregate(self, pipeline: list) -> List[dict]:
        return self._coll.aggregate(pipeline)

    def __iter__(self) -> Iterator[T]:
        for d in self._coll.find({}):
            yield _dict_to_model(self._model, d)


__all__ = ["Document"]
