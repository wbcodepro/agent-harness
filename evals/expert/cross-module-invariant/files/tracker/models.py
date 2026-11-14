"""The Task record."""

from dataclasses import dataclass, replace
from typing import Optional, Tuple

#: Allowed priorities. 1 is the most urgent, 4 the least.
PRIORITIES = (1, 2, 3, 4)


@dataclass(frozen=True)
class Task:
    """One tracked task.

    id        positive integer, unique within a store
    title     non-empty text
    priority  one of PRIORITIES; 1 is the most urgent
    due       day number the task is due on, or None for "no due day"
    tags      tuple of tag names
    revision  starts at 1 and goes up by one on every change
    """

    id: int
    title: str
    priority: int = 3
    due: Optional[int] = None
    tags: Tuple[str, ...] = ()
    revision: int = 1

    def __post_init__(self):
        if not isinstance(self.id, int) or self.id < 1:
            raise ValueError("id must be a positive integer")
        if not self.title:
            raise ValueError("title must not be empty")
        if self.priority not in PRIORITIES:
            raise ValueError("priority must be one of %r" % (PRIORITIES,))
        object.__setattr__(self, "tags", tuple(self.tags))

    def changed(self, **changes):
        """Return a copy with `changes` applied and the revision bumped."""
        if "id" in changes or "revision" in changes:
            raise ValueError("id and revision cannot be changed directly")
        return replace(self, revision=self.revision + 1, **changes)
