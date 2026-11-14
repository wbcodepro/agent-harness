import os
import sys
import threading
import unittest

import scheduler as scheduler_module
from scheduler import Scheduler

WAIT = 3          # generous upper bound for things that must happen
SHORT = 0.25      # how long we watch something that must not happen


def _give_up():
    print("FAILED: the check did not finish within 15 seconds", flush=True)
    os._exit(2)


_watchdog = threading.Timer(15, _give_up)
_watchdog.daemon = True
_watchdog.start()


class Blocker:
    """A job function that says when it started and waits to be released."""

    def __init__(self, value=None):
        self.started = threading.Event()
        self.release = threading.Event()
        self.value = value

    def __call__(self):
        self.started.set()
        self.release.wait(WAIT * 2)
        return self.value


class Case(unittest.TestCase):
    def make(self, workers=1, **hooks):
        made = Scheduler(workers=workers, **hooks)
        self.addCleanup(made.shutdown, False)
        return made

    def blocker(self, value=None):
        made = Blocker(value)
        self.addCleanup(made.release.set)
        return made

    def busy(self, made):
        """Occupy one worker of `made`; returns (blocker, its job)."""
        blocker = self.blocker("blocker")
        job = made.submit(blocker)
        self.assertTrue(blocker.started.wait(WAIT), "the first job never started")
        return blocker, job

    def assert_cancelled(self, job):
        self.assertEqual(job.state, "cancelled")
        try:
            job.result(SHORT)
        except scheduler_module.Cancelled:
            pass
        except TimeoutError:
            self.fail("result() of a cancelled job waited instead of raising Cancelled")
        else:
            self.fail("result() of a cancelled job returned")


class Existing(Case):
    def test_result_exception_and_states(self):
        made = self.make(workers=2)
        good = made.submit(pow, 2, 10)
        bad = made.submit(int, "x")
        self.assertEqual(good.result(WAIT), 1024)
        with self.assertRaises(ValueError):
            bad.result(WAIT)
        self.assertEqual((good.state, bad.state), ("done", "failed"))

    def test_hooks_for_an_ordinary_job(self):
        seen = []
        made = self.make(
            on_dequeue=lambda job: seen.append(("dequeue", job.state)),
            on_start=lambda job: seen.append(("start", job.state)),
            on_finish=lambda job: seen.append(("finish", job.state)))
        made.submit(len, "abc")
        self.assertTrue(made.join(WAIT))
        self.assertEqual(seen, [("dequeue", "pending"), ("start", "running"),
                                ("finish", "done")])

    def test_shutdown_runs_what_was_submitted(self):
        made = Scheduler(workers=2)
        ran = []
        for number in range(20):
            made.submit(ran.append, number)
        closer = threading.Thread(target=made.shutdown, daemon=True)
        closer.start()
        closer.join(WAIT)
        self.assertFalse(closer.is_alive(), "shutdown() did not return")
        self.assertEqual(sorted(ran), list(range(20)))
        with self.assertRaises(RuntimeError):
            made.submit(len, "abc")


class Priorities(Case):
    def test_highest_first_fifo_within_a_priority(self):
        made = self.make()
        blocker, _ = self.busy(made)
        order = []
        for name, priority in [("a", 0), ("b", 5), ("c", 0), ("d", 5),
                               ("e", -1), ("f", 5), ("g", 2), ("h", -1)]:
            made.submit(order.append, name, priority=priority)
        blocker.release.set()
        self.assertTrue(made.join(WAIT))
        self.assertEqual(order, ["b", "d", "f", "g", "a", "c", "e", "h"])

    def test_default_priority_is_zero(self):
        made = self.make()
        blocker, _ = self.busy(made)
        order = []
        made.submit(order.append, "low", priority=-1)
        made.submit(order.append, "default-1")
        made.submit(order.append, "high", priority=1)
        made.submit(order.append, "zero", priority=0)
        made.submit(order.append, "default-2")
        blocker.release.set()
        self.assertTrue(made.join(WAIT))
        self.assertEqual(order, ["high", "default-1", "zero", "default-2", "low"])

    def test_many_jobs_keep_submission_order(self):
        made = self.make()
        blocker, _ = self.busy(made)
        order = []
        for number in range(200):
            made.submit(order.append, number, priority=number % 3)
        blocker.release.set()
        self.assertTrue(made.join(WAIT))
        expected = [n for p in (2, 1, 0) for n in range(200) if n % 3 == p]
        self.assertEqual(order, expected)

    def test_priority_is_not_passed_to_the_function(self):
        made = self.make()
        job = made.submit(lambda *args, **kwargs: (args, kwargs), 1, 2,
                          priority=7, colour="red")
        self.assertEqual(job.result(WAIT), ((1, 2), {"colour": "red"}))

    def test_job_submitted_from_a_hook_is_ordered_too(self):
        order = []
        holder = {}

        def on_dequeue(job):
            if job is holder.get("first"):
                holder["made"].submit(order.append, "urgent", priority=10)
                holder["made"].submit(order.append, "late", priority=0)

        made = holder["made"] = self.make(on_dequeue=on_dequeue)
        blocker, _ = self.busy(made)
        holder["first"] = made.submit(order.append, "first", priority=5)
        made.submit(order.append, "low-1")
        made.submit(order.append, "low-2")
        blocker.release.set()
        for _ in range(3):      # the hook adds jobs while we wait
            self.assertTrue(made.join(WAIT))
        self.assertEqual(order, ["first", "urgent", "low-1", "low-2", "late"])


class Cancelling(Case):
    def test_pending_job(self):
        seen = []
        made = self.make(on_start=lambda job: seen.append(job),
                         on_finish=lambda job: seen.append(job))
        blocker, _ = self.busy(made)
        calls = []
        job = made.submit(calls.append, "ran")
        after = made.submit(calls.append, "after")
        self.assertIs(job.cancel(), True)
        self.assert_cancelled(job)          # while the worker is still busy
        blocker.release.set()
        self.assertTrue(made.join(WAIT))
        self.assertEqual(calls, ["after"])
        self.assert_cancelled(job)
        self.assertEqual(after.state, "done")
        self.assertNotIn(job, seen, "on_start/on_finish ran for a cancelled job")

    def test_running_job(self):
        made = self.make()
        blocker, job = self.busy(made)
        self.assertEqual(job.state, "running")
        self.assertIs(job.cancel(), False)
        self.assertEqual(job.state, "running")
        blocker.release.set()
        self.assertEqual(job.result(WAIT), "blocker")
        self.assertEqual(job.state, "done")

    def test_finished_jobs(self):
        made = self.make()
        good = made.submit(pow, 2, 5)
        bad = made.submit(int, "x")
        self.assertTrue(made.join(WAIT))
        self.assertIs(good.cancel(), False)
        self.assertIs(bad.cancel(), False)
        self.assertEqual((good.state, bad.state), ("done", "failed"))
        self.assertEqual(good.result(WAIT), 32)
        with self.assertRaises(ValueError):
            bad.result(WAIT)

    def test_cancelled_is_an_exception_class(self):
        self.assertTrue(issubclass(scheduler_module.Cancelled, BaseException))

    def test_thread_blocked_in_result_is_woken(self):
        made = self.make()
        blocker, _ = self.busy(made)
        job = made.submit(len, "abc")
        entered = threading.Event()
        outcome = []

        def waiter():
            entered.set()
            try:
                outcome.append(("returned", job.result(WAIT * 2)))
            except BaseException as error:
                outcome.append(("raised", type(error)))

        thread = threading.Thread(target=waiter, daemon=True)
        thread.start()
        self.assertTrue(entered.wait(WAIT))
        threading.Event().wait(0.1)         # let it block in result()
        self.assertIs(job.cancel(), True)
        thread.join(WAIT)
        self.assertFalse(thread.is_alive(),
                         "result() kept waiting after the job was cancelled")
        self.assertEqual(outcome, [("raised", scheduler_module.Cancelled)])

    def test_cancelling_one_of_many(self):
        made = self.make()
        blocker, _ = self.busy(made)
        order = []
        jobs = [made.submit(order.append, n, priority=n % 2) for n in range(10)]
        for number in (1, 4, 5, 8):
            self.assertIs(jobs[number].cancel(), True)
        blocker.release.set()
        self.assertTrue(made.join(WAIT))
        self.assertEqual(order, [3, 7, 9, 0, 2, 6])


class Waiting(Case):
    def test_join_does_not_wait_for_a_cancelled_job(self):
        made = self.make()
        blocker, _ = self.busy(made)
        calls = []
        doomed = made.submit(calls.append, "doomed")
        made.submit(calls.append, "kept")
        self.assertIs(doomed.cancel(), True)
        blocker.release.set()
        self.assertTrue(made.join(WAIT), "join() waits for a cancelled job")
        self.assertEqual(calls, ["kept"])

    def test_join_still_waits_for_the_running_job(self):
        made = self.make()
        blocker, _ = self.busy(made)
        doomed = made.submit(len, "abc")
        self.assertIs(doomed.cancel(), True)
        self.assertFalse(made.join(SHORT), "join() returned while a job was running")
        blocker.release.set()
        self.assertTrue(made.join(WAIT))

    def test_join_still_waits_for_jobs_after_a_cancelled_one(self):
        made = self.make()
        first, _ = self.busy(made)
        doomed = made.submit(len, "abc")
        second = self.blocker()
        made.submit(second)
        self.assertIs(doomed.cancel(), True)
        first.release.set()
        self.assertTrue(second.started.wait(WAIT), "the job after a cancelled one never started")
        self.assertFalse(made.join(SHORT), "join() returned while a job was running")
        second.release.set()
        self.assertTrue(made.join(WAIT))

    def test_shutdown_does_not_wait_for_a_cancelled_job(self):
        made = Scheduler(workers=1)
        blocker, _ = self.busy(made)
        calls = []
        doomed = made.submit(calls.append, "doomed")
        made.submit(calls.append, "kept")
        self.assertIs(doomed.cancel(), True)
        closer = threading.Thread(target=made.shutdown, daemon=True)
        closer.start()
        closer.join(SHORT)
        self.assertTrue(closer.is_alive(), "shutdown() returned while a job was running")
        blocker.release.set()
        closer.join(WAIT)
        self.assertFalse(closer.is_alive(), "shutdown() did not return")
        self.assertEqual(calls, ["kept"])


class Truthful(Case):
    """cancel() must tell the truth at the moments a hook makes reachable."""

    def settle(self, made, job, answer, calls):
        self.assertTrue(made.join(WAIT), "the scheduler never became idle")
        self.assertIn(answer, (True, False))
        if answer:
            self.assertEqual(calls, ["after"],
                             "cancel() returned True but the job's function was called")
            self.assert_cancelled(job)
        else:
            self.assertEqual(calls, ["ran", "after"],
                             "cancel() returned False but the job did not run")
            self.assertEqual(job.state, "done")
            self.assertIsNone(job.result(WAIT))

    def test_cancel_inside_on_dequeue(self):
        holder = {}

        def on_dequeue(job):
            if job is holder.get("target"):
                holder["answer"] = job.cancel()

        made = self.make(on_dequeue=on_dequeue)
        blocker, _ = self.busy(made)
        calls = []
        target = holder["target"] = made.submit(calls.append, "ran")
        made.submit(calls.append, "after")
        blocker.release.set()
        self.assertTrue(made.join(WAIT), "the scheduler never became idle")
        self.assertIn("answer", holder, "on_dequeue was not called for the job")
        self.settle(made, target, holder["answer"], calls)

    def test_cancel_from_another_thread_during_on_dequeue(self):
        holder = {}
        parked = threading.Event()
        proceed = threading.Event()
        self.addCleanup(proceed.set)

        def on_dequeue(job):
            if job is holder.get("target"):
                parked.set()
                proceed.wait(WAIT * 2)

        made = self.make(on_dequeue=on_dequeue)
        blocker, _ = self.busy(made)
        calls = []
        target = holder["target"] = made.submit(calls.append, "ran")
        made.submit(calls.append, "after")
        blocker.release.set()
        self.assertTrue(parked.wait(WAIT), "on_dequeue was not called for the job")
        answer = target.cancel()
        proceed.set()
        self.settle(made, target, answer, calls)

    def test_cancel_inside_on_start_is_too_late(self):
        holder = {}

        def on_start(job):
            if job is holder.get("target"):
                holder["answer"] = job.cancel()

        made = self.make(on_start=on_start)
        blocker, _ = self.busy(made)
        calls = []
        target = holder["target"] = made.submit(calls.append, "ran")
        made.submit(calls.append, "after")
        blocker.release.set()
        self.assertTrue(made.join(WAIT), "the scheduler never became idle")
        self.assertIs(holder.get("answer"), False)
        self.settle(made, target, False, calls)

    def test_many_threads(self):
        ran = []
        made = self.make(workers=4, on_dequeue=lambda job: None)
        blockers = [self.busy(made)[0] for _ in range(4)]
        jobs = [made.submit(ran.append, n, priority=n % 5) for n in range(400)]
        answers = [None] * len(jobs)

        def canceller(numbers):
            for number in numbers:
                answers[number] = jobs[number].cancel()

        threads = [threading.Thread(target=canceller, args=(range(k, 400, 6),),
                                    daemon=True) for k in range(3)]
        for blocker in blockers:
            blocker.release.set()
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(WAIT)
            self.assertFalse(thread.is_alive())
        self.assertTrue(made.join(WAIT), "the scheduler never became idle")
        self.assertEqual(len(ran), len(set(ran)), "a job ran twice")
        done = set(ran)
        for number, job in enumerate(jobs):
            if answers[number] is True:
                self.assertNotIn(number, done)
                self.assertEqual(job.state, "cancelled")
            else:
                self.assertIn(number, done)
                self.assertEqual(job.state, "done")


if __name__ == "__main__":
    program = unittest.main(exit=False)
    sys.stdout.flush()
    sys.stderr.flush()
    # Leave at once: a scheduler under test may have left threads behind.
    os._exit(0 if program.result.wasSuccessful() else 1)
