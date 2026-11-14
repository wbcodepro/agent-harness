"""What changed between two snapshots of a store.

Take `before = store.snapshot()`, let time pass, take
`after = store.snapshot()`, then `diff(before, after)` tells which tasks
were added, removed or changed in between.
"""

from collections import namedtuple

#: Three lists of task ids, each in ascending order.
Changes = namedtuple("Changes", "added removed changed")


def diff(before, after):
    """Compare two snapshots taken with TaskStore.snapshot().

    Returns Changes(added, removed, changed):

        added    ids present only in `after`
        removed  ids present only in `before`
        changed  ids present in both whose revision differs

    Each list is in ascending id order. The two snapshots are walked side
    by side in a single pass.
    """
    added, removed, changed = [], [], []
    i = j = 0
    while i < len(before) and j < len(after):
        old_id, old_revision = before[i]
        new_id, new_revision = after[j]
        if old_id == new_id:
            if old_revision != new_revision:
                changed.append(old_id)
            i += 1
            j += 1
        elif old_id < new_id:
            removed.append(old_id)
            i += 1
        else:
            added.append(new_id)
            j += 1
    removed.extend(task_id for task_id, _ in before[i:])
    added.extend(task_id for task_id, _ in after[j:])
    return Changes(added, removed, changed)


def summary(before, after):
    """Return a one-line description of diff(before, after)."""
    changes = diff(before, after)
    if not any(changes):
        return "no changes"
    parts = []
    for label, ids in zip(("added", "removed", "changed"), changes):
        if ids:
            parts.append("%s %s" % (label, ", ".join("#%d" % i for i in ids)))
    return "; ".join(parts)
