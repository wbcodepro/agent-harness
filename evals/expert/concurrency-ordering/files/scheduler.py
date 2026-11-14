"""A small job scheduler on a pool of worker threads.

    scheduler = Scheduler(workers=2)
    job = scheduler.submit(fetch, url, retries=3)
    print(job.result())
    scheduler.shutdown()

Jobs start in the order they were submitted. A Job has a `state`:

    "pending"   submitted, not started yet
    "running"   its function is being called
    "done"      its function returned; result() returns the value
    "failed"    its function raised; result() raises the same exception

Tracing hooks
-------------
Three optional callables can be given to the Scheduler. Each is called on
the worker thread with the job as its only argument, and never while the
scheduler holds one of its own locks, so a hook is free to call back into
the scheduler or the job (submit(), result() with a timeout, ...).

    on_dequeue(job)  right after a worker has taken the job off the queue,
                     before the job starts; job.state is still "pending"
    on_start(job)    immediately before the job's function is called;
                     job.state is "running"
    on_finish(job)   after the function has returned or raised;
                     job.state is "done" or "failed"
"""

import collections
import threading


class Job:
    """Handle for one submitted call."""

    def __init__(self, fn, args, kwargs):
        self._fn = fn
        self._args = args
        self._kwargs = kwargs
        self._state = "pending"
        self._value = None
        self._error = None
        self._finished = threading.Event()

    @property
    def state(self):
        return self._state

    def result(self, timeout=None):
        """Wait for the job and return what its function returned.

        Raises the function's exception if it raised, and TimeoutError if
        the job has not finished within `timeout` seconds.
        """
        if not self._finished.wait(timeout):
            raise TimeoutError("job not finished")
        if self._error is not None:
            raise self._error
        return self._value


class Scheduler:
    """Runs submitted jobs on `workers` threads."""

    def __init__(self, workers=2, on_dequeue=None, on_start=None, on_finish=None):
        if workers < 1:
            raise ValueError("workers must be at least 1")
        self._on_dequeue = on_dequeue
        self._on_start = on_start
        self._on_finish = on_finish
        self._lock = threading.Lock()
        self._wakeup = threading.Condition(self._lock)
        self._all_done = threading.Condition(self._lock)
        self._queue = collections.deque()
        self._unfinished = 0
        self._closed = False
        self._threads = []
        for number in range(workers):
            thread = threading.Thread(
                target=self._work, name="scheduler-%d" % number, daemon=True)
            thread.start()
            self._threads.append(thread)

    def submit(self, fn, *args, **kwargs):
        """Queue fn(*args, **kwargs) and return its Job."""
        job = Job(fn, args, kwargs)
        with self._lock:
            if self._closed:
                raise RuntimeError("scheduler is shut down")
            self._queue.append(job)
            self._unfinished += 1
            self._wakeup.notify()
        return job

    def join(self, timeout=None):
        """Wait until every job submitted so far has finished.

        Returns True when nothing is pending or running any more, False if
        `timeout` seconds passed first.
        """
        with self._lock:
            return self._all_done.wait_for(
                lambda: self._unfinished == 0, timeout)

    def shutdown(self, wait=True):
        """Stop accepting jobs. Jobs already submitted still run.

        With wait=True, return only after they have all finished and the
        worker threads have ended.
        """
        with self._lock:
            self._closed = True
            self._wakeup.notify_all()
        if wait:
            for thread in self._threads:
                thread.join()

    def _take(self):
        """Block until a job is available; None means the worker should end."""
        with self._lock:
            while not self._queue and not self._closed:
                self._wakeup.wait()
            if not self._queue:
                return None
            return self._queue.popleft()

    def _work(self):
        while True:
            job = self._take()
            if job is None:
                return
            if self._on_dequeue is not None:
                self._on_dequeue(job)
            job._state = "running"
            if self._on_start is not None:
                self._on_start(job)
            try:
                job._value = job._fn(*job._args, **job._kwargs)
                job._state = "done"
            except Exception as error:
                job._error = error
                job._state = "failed"
            job._finished.set()
            if self._on_finish is not None:
                self._on_finish(job)
            with self._lock:
                self._unfinished -= 1
                if self._unfinished == 0:
                    self._all_done.notify_all()
