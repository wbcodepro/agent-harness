import threading
import unittest

from scheduler import Scheduler

WAIT = 5


class Basics(unittest.TestCase):
    def test_result_and_state(self):
        scheduler = Scheduler(workers=2)
        job = scheduler.submit(pow, 2, 10)
        self.assertEqual(job.result(WAIT), 1024)
        self.assertEqual(job.state, "done")
        scheduler.shutdown()

    def test_keyword_arguments(self):
        scheduler = Scheduler(workers=1)
        job = scheduler.submit(sorted, [3, 1, 2], reverse=True)
        self.assertEqual(job.result(WAIT), [3, 2, 1])
        scheduler.shutdown()

    def test_exception_is_reraised(self):
        scheduler = Scheduler(workers=1)
        job = scheduler.submit(int, "not a number")
        with self.assertRaises(ValueError):
            job.result(WAIT)
        self.assertEqual(job.state, "failed")
        scheduler.shutdown()

    def test_jobs_start_in_submission_order(self):
        gate = threading.Event()
        order = []
        scheduler = Scheduler(workers=1)
        scheduler.submit(gate.wait, WAIT)
        for name in "abcde":
            scheduler.submit(order.append, name)
        gate.set()
        self.assertTrue(scheduler.join(WAIT))
        self.assertEqual(order, list("abcde"))
        scheduler.shutdown()

    def test_join_times_out_while_a_job_runs(self):
        gate = threading.Event()
        scheduler = Scheduler(workers=1)
        scheduler.submit(gate.wait, WAIT)
        self.assertFalse(scheduler.join(0.05))
        gate.set()
        self.assertTrue(scheduler.join(WAIT))
        scheduler.shutdown()

    def test_hooks(self):
        seen = []
        scheduler = Scheduler(
            workers=1,
            on_dequeue=lambda job: seen.append(("dequeue", job.state)),
            on_start=lambda job: seen.append(("start", job.state)),
            on_finish=lambda job: seen.append(("finish", job.state)),
        )
        scheduler.submit(len, "abc")
        self.assertTrue(scheduler.join(WAIT))
        scheduler.shutdown()
        self.assertEqual(seen, [("dequeue", "pending"), ("start", "running"),
                                ("finish", "done")])

    def test_submit_after_shutdown(self):
        scheduler = Scheduler(workers=1)
        scheduler.shutdown()
        with self.assertRaises(RuntimeError):
            scheduler.submit(len, "abc")


if __name__ == "__main__":
    unittest.main()
