"""TaskStore: the container the rest of the package reads from."""

from .models import Task


class DuplicateTask(Exception):
    """A task with this id is already in the store."""


class UnknownTask(KeyError):
    """No task with this id is in the store."""


class TaskStore:
    """Holds tasks by id."""

    def __init__(self, tasks=()):
        self._tasks = {}
        for task in tasks:
            self.add(task)

    def __len__(self):
        return len(self._tasks)

    def __contains__(self, task_id):
        return task_id in self._tasks

    def __iter__(self):
        return iter(self.all())

    def all(self):
        """Return every task as a new list, ordered by id."""
        return sorted(self._tasks.values(), key=lambda task: task.id)

    def get(self, task_id):
        """Return the task with this id or raise UnknownTask."""
        try:
            return self._tasks[task_id]
        except KeyError:
            raise UnknownTask(task_id) from None

    def add(self, task):
        """Add an existing Task; its id must not be taken."""
        if task.id in self._tasks:
            raise DuplicateTask(task.id)
        self._tasks[task.id] = task
        return task

    def next_id(self):
        """Return the id a new task would get: one more than the highest
        id in the store, or 1 for an empty store."""
        tasks = self.all()
        if not tasks:
            return 1
        return tasks[-1].id + 1

    def create(self, title, priority=3, due=None, tags=()):
        """Create a task with the next free id, add it and return it."""
        task = Task(self.next_id(), title, priority=priority, due=due, tags=tags)
        return self.add(task)

    def update(self, task_id, **changes):
        """Apply `changes` to a task, bump its revision, return the new task."""
        task = self.get(task_id).changed(**changes)
        self._tasks[task_id] = task
        return task

    def remove(self, task_id):
        """Remove and return the task with this id or raise UnknownTask."""
        try:
            return self._tasks.pop(task_id)
        except KeyError:
            raise UnknownTask(task_id) from None

    def snapshot(self):
        """Return one (id, revision) pair per task.

        A snapshot is cheap to keep around; digest.diff() compares two of
        them to tell what happened in between.
        """
        return [(task.id, task.revision) for task in self]
