"""Simple queries over a store.

Every function returns a set of task ids, so callers that care about an
order pick their own.
"""


def with_tag(store, tag):
    """Ids of the tasks carrying `tag`."""
    return {task.id for task in store if tag in task.tags}


def overdue(store, today):
    """Ids of the tasks whose due day is before `today`."""
    return {
        task.id for task in store
        if task.due is not None and task.due < today
    }


def urgent(store, at_most=1):
    """Ids of the tasks whose priority is `at_most` or more urgent."""
    return {task.id for task in store if task.priority <= at_most}


def count_by_priority(store):
    """Return {priority: number of tasks} for the priorities in use."""
    counts = {}
    for task in store:
        counts[task.priority] = counts.get(task.priority, 0) + 1
    return counts
