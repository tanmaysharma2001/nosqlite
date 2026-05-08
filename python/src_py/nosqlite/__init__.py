"""NoSQLite — a MongoDB-style document database backed by SQLite.

Example:
    >>> import nosqlite
    >>> db = nosqlite.Database()                # in-memory
    >>> users = db.collection("users")
    >>> users.insert_one({"name": "Alice", "age": 30})
    >>> users.find({"age": {"$gt": 25}})
    [{'name': 'Alice', 'age': 30, '_id': '...'}]
"""

from ._nosqlite import Database, Collection, Transaction

__all__ = ["Database", "Collection", "Transaction"]
__version__ = "0.1.0"
