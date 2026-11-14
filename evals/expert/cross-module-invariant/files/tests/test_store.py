import unittest

from tracker import DuplicateTask, Task, TaskStore, UnknownTask


def ids(tasks):
    return [task.id for task in tasks]


class Basics(unittest.TestCase):
    def test_add_and_get(self):
        store = TaskStore([Task(1, "write")])
        self.assertEqual(store.get(1).title, "write")
        self.assertEqual(len(store), 1)
        with self.assertRaises(DuplicateTask):
            store.add(Task(1, "again"))
        with self.assertRaises(UnknownTask):
            store.get(2)

    def test_update_bumps_revision(self):
        store = TaskStore([Task(1, "write")])
        self.assertEqual(store.update(1, title="rewrite").revision, 2)


class WorkingOrder(unittest.TestCase):
    def test_most_urgent_first(self):
        store = TaskStore([
            Task(1, "low", priority=4),
            Task(2, "top", priority=1),
            Task(3, "mid", priority=2),
        ])
        self.assertEqual(ids(store.all()), [2, 3, 1])

    def test_earliest_due_first_within_a_priority(self):
        store = TaskStore([
            Task(1, "later", priority=2, due=30),
            Task(2, "sooner", priority=2, due=12),
            Task(3, "urgent", priority=1, due=99),
        ])
        self.assertEqual(ids(store.all()), [3, 2, 1])

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
            Task(6, "b", priority=2, due=5),
        ])
        self.assertEqual(ids(store.all()), [4, 6, 9])


if __name__ == "__main__":
    unittest.main()
