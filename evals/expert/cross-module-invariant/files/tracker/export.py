"""CSV export and import."""

import csv
import io

from .models import Task
from .store import TaskStore

HEADER = ["id", "title", "priority", "due", "tags"]


def to_csv(store):
    """Return the store as CSV text: a header line, then one row per task.

    Rows are in ascending id order, so that two exports of nearly the same
    store differ only in the rows that really changed. `due` is empty for a
    task without a due day; `tags` are joined with single spaces. Lines end
    with "\\n".
    """
    out = io.StringIO()
    writer = csv.writer(out, lineterminator="\n")
    writer.writerow(HEADER)
    for task in store.all():
        writer.writerow([
            task.id,
            task.title,
            task.priority,
            "" if task.due is None else task.due,
            " ".join(task.tags),
        ])
    return out.getvalue()


def from_csv(text):
    """Build a TaskStore from text written by to_csv()."""
    reader = csv.reader(io.StringIO(text))
    header = next(reader, None)
    if header != HEADER:
        raise ValueError("not a tracker export")
    store = TaskStore()
    for row in reader:
        if not row:
            continue
        task_id, title, priority, due, tags = row
        store.add(Task(
            int(task_id),
            title,
            priority=int(priority),
            due=int(due) if due else None,
            tags=tuple(tags.split()),
        ))
    return store
