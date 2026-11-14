"""Cursor-based paging over a store.

A client asks for "the next `limit` tasks after the last id I have seen".
Because the cursor is an id, a page stays correct when tasks are added or
removed between two requests.
"""

import bisect


def page(store, after_id=0, limit=10):
    """Return up to `limit` tasks whose id is greater than `after_id`.

    The tasks come in ascending id order, starting with the smallest id
    above the cursor. An empty list means there is nothing after the cursor.
    """
    if limit < 1:
        raise ValueError("limit must be at least 1")
    tasks = list(store)
    ids = [task.id for task in tasks]
    start = bisect.bisect_right(ids, after_id)
    return tasks[start:start + limit]


def pages(store, limit=10):
    """Yield every page of the store in turn, each a non-empty list.

    Together the pages hold every task exactly once, in ascending id order.
    """
    cursor = 0
    while True:
        chunk = page(store, after_id=cursor, limit=limit)
        if not chunk:
            return
        yield chunk
        cursor = chunk[-1].id


def page_count(store, limit=10):
    """Return how many pages pages() would yield."""
    if limit < 1:
        raise ValueError("limit must be at least 1")
    return (len(store) + limit - 1) // limit
