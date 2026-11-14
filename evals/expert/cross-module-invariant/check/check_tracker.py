import os
import threading
import unittest

from tracker import Task, TaskStore
from tracker import digest, export, filters, paging

# A broken pager can loop for ever; fail instead of hanging.
def _give_up():
    print("FAILED: the check did not finish within 8 seconds", flush=True)
    os._exit(2)


_watchdog = threading.Timer(8, _give_up)
_watchdog.daemon = True
_watchdog.start()


def ids(tasks):
    return [task.id for task in tasks]


def sample():
    """Ten tasks whose working order is nothing like their id order."""
    return TaskStore([
        Task(1, "archive", priority=4),
        Task(2, "invoice", priority=2, due=20),
        Task(3, "backup", priority=3, due=5),
        Task(4, "outage", priority=1, due=9),
        Task(5, "review", priority=2, due=11),
        Task(6, "tidy", priority=4, due=2),
        Task(7, "hotfix", priority=1),
        Task(8, "plan", priority=3),
        Task(9, "call", priority=2),
        Task(10, "audit", priority=1, due=3, tags=("legal", "q3")),
    ])


class WorkingOrder(unittest.TestCase):
    def test_full_order(self):
        self.assertEqual(ids(sample().all()), [10, 4, 7, 5, 2, 9, 3, 8, 6, 1])

    def test_no_due_day_goes_last_within_a_priority(self):
        store = TaskStore([
            Task(1, "whenever", priority=2),
            Task(2, "dated", priority=2, due=40),
            Task(3, "low but dated", priority=3, due=1),
        ])
        self.assertEqual(ids(store.all()), [2, 1, 3])

    def test_remaining_ties_by_id(self):
        store = TaskStore([
            Task(9, "c", priority=2, due=5),
            Task(4, "a", priority=2, due=5),
            Task(6, "b", priority=2),
            Task(5, "d", priority=2),
        ])
        self.assertEqual(ids(store.all()), [4, 9, 5, 6])

    def test_order_follows_updates(self):
        store = sample()
        store.update(1, priority=1, due=1)
        self.assertEqual(ids(store.all())[:2], [1, 10])

    def test_all_returns_a_new_list(self):
        store = sample()
        store.all().clear()
        self.assertEqual(len(store.all()), 10)


class Creating(unittest.TestCase):
    def test_new_id_is_one_more_than_the_highest(self):
        store = sample()
        self.assertEqual(store.next_id(), 11)
        task = store.create("new", priority=1, due=1)
        self.assertEqual(task.id, 11)
        self.assertEqual(store.create("another", priority=4).id, 12)
        self.assertEqual(len(store), 12)

    def test_first_id_is_one(self):
        self.assertEqual(TaskStore().create("first").id, 1)

    def test_gaps_are_not_reused(self):
        store = TaskStore([Task(3, "a", priority=4), Task(7, "b", priority=4),
                           Task(5, "c", priority=1)])
        self.assertEqual(store.create("d").id, 8)


class Paging(unittest.TestCase):
    def test_first_page_is_the_lowest_ids(self):
        self.assertEqual(ids(paging.page(sample(), limit=4)), [1, 2, 3, 4])

    def test_page_after_a_cursor(self):
        self.assertEqual(ids(paging.page(sample(), after_id=4, limit=3)), [5, 6, 7])
        self.assertEqual(ids(paging.page(sample(), after_id=8, limit=5)), [9, 10])
        self.assertEqual(paging.page(sample(), after_id=10, limit=5), [])

    def test_cursor_need_not_be_an_existing_id(self):
        store = sample()
        store.remove(5)
        self.assertEqual(ids(paging.page(store, after_id=5, limit=2)), [6, 7])

    def test_pages_cover_everything_once_in_id_order(self):
        chunks = list(paging.pages(sample(), limit=3))
        self.assertEqual([ids(c) for c in chunks],
                         [[1, 2, 3], [4, 5, 6], [7, 8, 9], [10]])
        self.assertEqual(paging.page_count(sample(), limit=3), 4)

    def test_pages_of_one(self):
        store = TaskStore([Task(2, "b", priority=1), Task(1, "a", priority=4),
                           Task(3, "c", priority=2)])
        self.assertEqual([ids(c) for c in paging.pages(store, limit=1)],
                         [[1], [2], [3]])


class Digest(unittest.TestCase):
    def test_nothing_changed(self):
        store = sample()
        changes = digest.diff(store.snapshot(), store.snapshot())
        self.assertEqual(tuple(changes), ([], [], []))
        self.assertEqual(digest.summary(store.snapshot(), store.snapshot()),
                         "no changes")

    def test_one_title_change(self):
        store = sample()
        before = store.snapshot()
        store.update(8, title="plan the quarter")
        changes = digest.diff(before, store.snapshot())
        self.assertEqual(tuple(changes), ([], [], [8]))

    def test_reprioritising_is_one_change(self):
        store = sample()
        before = store.snapshot()
        store.update(1, priority=1, due=1)
        after = store.snapshot()
        self.assertEqual(tuple(digest.diff(before, after)), ([], [], [1]))
        self.assertEqual(digest.summary(before, after), "changed #1")

    def test_added_removed_and_changed(self):
        store = sample()
        before = store.snapshot()
        store.remove(4)
        store.remove(9)
        store.add(Task(12, "late", priority=1, due=1))
        store.add(Task(11, "later", priority=4))
        store.update(6, priority=1)
        store.update(2, due=1)
        after = store.snapshot()
        changes = digest.diff(before, after)
        self.assertEqual(changes.added, [11, 12])
        self.assertEqual(changes.removed, [4, 9])
        self.assertEqual(changes.changed, [2, 6])
        self.assertEqual(digest.summary(before, after),
                         "added #11, #12; removed #4, #9; changed #2, #6")


class Export(unittest.TestCase):
    def test_rows_in_id_order(self):
        text = export.to_csv(sample())
        lines = text.splitlines()
        self.assertEqual(lines[0], "id,title,priority,due,tags")
        self.assertEqual([int(line.split(",")[0]) for line in lines[1:]],
                         list(range(1, 11)))
        self.assertEqual(lines[10], "10,audit,1,3,legal q3")
        self.assertEqual(lines[1], "1,archive,4,,")

    def test_round_trip(self):
        again = export.from_csv(export.to_csv(sample()))
        self.assertEqual(again.all(), sample().all())
        self.assertEqual(ids(again.all()), [10, 4, 7, 5, 2, 9, 3, 8, 6, 1])


class Filters(unittest.TestCase):
    def test_queries(self):
        store = sample()
        self.assertEqual(filters.with_tag(store, "legal"), {10})
        self.assertEqual(filters.overdue(store, 6), {3, 6, 10})
        self.assertEqual(filters.urgent(store), {4, 7, 10})
        self.assertEqual(filters.count_by_priority(store), {1: 3, 2: 3, 3: 2, 4: 2})


if __name__ == "__main__":
    unittest.main()
