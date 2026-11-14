# tracker

A tiny in-memory task tracker.

    from tracker import Task, TaskStore
    from tracker import paging, digest, export

    store = TaskStore()
    store.create("write the report", priority=2, due=14)

    before = store.snapshot()
    store.update(1, title="write the annual report")
    print(digest.summary(before, store.snapshot()))   # changed #1

    for chunk in paging.pages(store, limit=20):
        ...

    text = export.to_csv(store)

Run the tests with

    python3 -m unittest discover -s tests -q
