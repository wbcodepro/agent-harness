package pool

import (
	"context"
	"errors"
	"runtime"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

const patience = 5 * time.Second

// settle waits for the goroutine count to come back to the baseline.
func settle(t *testing.T, baseline int) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for {
		n := runtime.NumGoroutine()
		if n <= baseline {
			return
		}
		if time.Now().After(deadline) {
			buf := make([]byte, 1<<16)
			buf = buf[:runtime.Stack(buf, true)]
			t.Fatalf("goroutines left behind: %d running, %d before Run\n%s", n, baseline, buf)
		}
		time.Sleep(2 * time.Millisecond)
	}
}

// wait receives from ch or fails the test.
func wait(t *testing.T, ch <-chan struct{}, what string) {
	t.Helper()
	select {
	case <-ch:
	case <-time.After(patience):
		t.Fatalf("timed out waiting for %s", what)
	}
}

// start runs Run in the background and hands back its result channel.
func start(ctx context.Context, workers int, tasks []Task) <-chan error {
	out := make(chan error, 1)
	go func() { out <- Run(ctx, workers, tasks) }()
	return out
}

func result(t *testing.T, out <-chan error) error {
	t.Helper()
	select {
	case err := <-out:
		return err
	case <-time.After(patience):
		t.Fatalf("Run did not return")
		return nil
	}
}

func TestRunsEveryTask(t *testing.T) {
	baseline := runtime.NumGoroutine()
	var ran atomic.Int64
	tasks := make([]Task, 50)
	for i := range tasks {
		tasks[i] = func(ctx context.Context) error {
			ran.Add(1)
			return nil
		}
	}
	if err := result(t, start(context.Background(), 4, tasks)); err != nil {
		t.Fatalf("Run returned %v", err)
	}
	if ran.Load() != 50 {
		t.Fatalf("ran %d tasks, want 50", ran.Load())
	}
	settle(t, baseline)
}

func TestNoTasks(t *testing.T) {
	baseline := runtime.NumGoroutine()
	if err := result(t, start(context.Background(), 3, nil)); err != nil {
		t.Fatalf("Run returned %v", err)
	}
	settle(t, baseline)
}

func TestNoWorkers(t *testing.T) {
	baseline := runtime.NumGoroutine()
	ran := false
	tasks := []Task{func(ctx context.Context) error { ran = true; return nil }}
	for _, workers := range []int{0, -3} {
		if err := result(t, start(context.Background(), workers, tasks)); !errors.Is(err, ErrNoWorkers) {
			t.Fatalf("Run with %d workers returned %v, want ErrNoWorkers", workers, err)
		}
	}
	settle(t, baseline)
	if ran {
		t.Fatalf("a task ran although there were no workers")
	}
}

func TestOneWorkerRunsInOrder(t *testing.T) {
	baseline := runtime.NumGoroutine()
	var mu sync.Mutex
	var order []int
	var active, overlap atomic.Int64
	tasks := make([]Task, 30)
	for i := range tasks {
		i := i
		tasks[i] = func(ctx context.Context) error {
			if active.Add(1) > 1 {
				overlap.Add(1)
			}
			runtime.Gosched()
			mu.Lock()
			order = append(order, i)
			mu.Unlock()
			active.Add(-1)
			return nil
		}
	}
	if err := result(t, start(context.Background(), 1, tasks)); err != nil {
		t.Fatalf("Run returned %v", err)
	}
	if overlap.Load() != 0 {
		t.Fatalf("tasks overlapped with one worker")
	}
	for i, got := range order {
		if got != i {
			t.Fatalf("order is %v", order)
		}
	}
	if len(order) != 30 {
		t.Fatalf("ran %d tasks, want 30", len(order))
	}
	settle(t, baseline)
}

// Exactly `workers` tasks run at once: no more, and no fewer when there is
// enough work.
func TestConcurrencyLimit(t *testing.T) {
	baseline := runtime.NumGoroutine()
	const workers = 3
	started := make(chan struct{}, 100)
	release := make(chan struct{})
	var active, peak atomic.Int64
	tasks := make([]Task, 12)
	for i := range tasks {
		tasks[i] = func(ctx context.Context) error {
			n := active.Add(1)
			for {
				p := peak.Load()
				if n <= p || peak.CompareAndSwap(p, n) {
					break
				}
			}
			started <- struct{}{}
			<-release
			active.Add(-1)
			return nil
		}
	}
	out := start(context.Background(), workers, tasks)
	for i := 0; i < workers; i++ {
		wait(t, started, "three tasks to run at the same time")
	}
	select {
	case <-started:
		t.Fatalf("a fourth task started while three were running")
	case <-time.After(100 * time.Millisecond):
	}
	// Let them through one at a time; each release lets one more start.
	for i := 0; i < len(tasks); i++ {
		release <- struct{}{}
		if i < len(tasks)-workers {
			wait(t, started, "the next task to start")
		}
	}
	if err := result(t, out); err != nil {
		t.Fatalf("Run returned %v", err)
	}
	if peak.Load() != workers {
		t.Fatalf("peak concurrency %d, want %d", peak.Load(), workers)
	}
	settle(t, baseline)
}

// A failing task cancels the context of the others, nothing new starts, and
// Run waits for what is still running.
func TestErrorStopsThePool(t *testing.T) {
	baseline := runtime.NumGoroutine()
	boom := errors.New("boom")
	var started atomic.Int64
	running := make(chan struct{}, 10)
	fail := make(chan struct{})
	sawCancel := make(chan struct{})
	finish := make(chan struct{})
	var finished atomic.Bool
	tasks := []Task{
		func(ctx context.Context) error {
			started.Add(1)
			running <- struct{}{}
			<-fail
			return boom
		},
		func(ctx context.Context) error {
			started.Add(1)
			running <- struct{}{}
			<-ctx.Done()
			close(sawCancel)
			<-finish // slow to stop: Run must wait for it
			finished.Store(true)
			return nil
		},
	}
	for i := 0; i < 8; i++ {
		tasks = append(tasks, func(ctx context.Context) error {
			started.Add(1)
			return nil
		})
	}
	out := start(context.Background(), 2, tasks)
	wait(t, running, "the first task")
	wait(t, running, "the second task")
	close(fail)
	wait(t, sawCancel, "the running task's context to be cancelled after another task failed")
	select {
	case err := <-out:
		t.Fatalf("Run returned %v while a task was still running", err)
	case <-time.After(150 * time.Millisecond):
	}
	close(finish)
	err := result(t, out)
	if err != boom {
		t.Fatalf("Run returned %v, want boom", err)
	}
	if !finished.Load() {
		t.Fatalf("Run returned before a running task had finished")
	}
	if n := started.Load(); n != 2 {
		t.Fatalf("%d tasks were started, want 2: nothing may start after a failure", n)
	}
	settle(t, baseline)
}

// The first error in time wins, not the one from the earliest task in the
// list and not a later one.
func TestFirstErrorWins(t *testing.T) {
	baseline := runtime.NumGoroutine()
	errA, errB, errC := errors.New("a"), errors.New("b"), errors.New("c")
	running := make(chan struct{}, 3)
	fail := make(chan struct{})
	aDone := make(chan struct{})
	tasks := []Task{
		func(ctx context.Context) error {
			running <- struct{}{}
			<-ctx.Done()
			defer close(aDone)
			return errA
		},
		func(ctx context.Context) error {
			running <- struct{}{}
			<-ctx.Done()
			<-aDone
			return errB
		},
		func(ctx context.Context) error {
			running <- struct{}{}
			<-fail
			return errC
		},
	}
	out := start(context.Background(), 3, tasks)
	for i := 0; i < 3; i++ {
		wait(t, running, "all three tasks to start")
	}
	close(fail)
	if err := result(t, out); err != errC {
		t.Fatalf("Run returned %v, want c (the first failure)", err)
	}
	settle(t, baseline)
}

// Cancelling the caller's context stops the pool: running tasks see it,
// nothing new starts, and Run reports the context's error.
func TestCancelStopsThePool(t *testing.T) {
	baseline := runtime.NumGoroutine()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var started atomic.Int64
	running := make(chan struct{}, 20)
	tasks := make([]Task, 20)
	for i := range tasks {
		tasks[i] = func(ctx context.Context) error {
			started.Add(1)
			running <- struct{}{}
			<-ctx.Done()
			return nil
		}
	}
	out := start(ctx, 4, tasks)
	for i := 0; i < 4; i++ {
		wait(t, running, "four tasks to start")
	}
	cancel()
	err := result(t, out)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("Run returned %v, want context.Canceled", err)
	}
	if n := started.Load(); n != 4 {
		t.Fatalf("%d tasks were started, want 4: nothing may start after cancellation", n)
	}
	settle(t, baseline)
}

func TestAlreadyCancelled(t *testing.T) {
	baseline := runtime.NumGoroutine()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	var started atomic.Int64
	tasks := make([]Task, 5)
	for i := range tasks {
		tasks[i] = func(ctx context.Context) error {
			started.Add(1)
			return nil
		}
	}
	if err := result(t, start(ctx, 2, tasks)); !errors.Is(err, context.Canceled) {
		t.Fatalf("Run returned %v, want context.Canceled", err)
	}
	if started.Load() != 0 {
		t.Fatalf("%d tasks ran under a cancelled context", started.Load())
	}
	settle(t, baseline)
}

func TestDeadline(t *testing.T) {
	baseline := runtime.NumGoroutine()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	tasks := make([]Task, 6)
	for i := range tasks {
		tasks[i] = func(ctx context.Context) error {
			<-ctx.Done()
			return nil
		}
	}
	if err := result(t, start(ctx, 2, tasks)); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("Run returned %v, want context.DeadlineExceeded", err)
	}
	settle(t, baseline)
}

// Every task ran and returned nil, so the result is nil even though the
// context was cancelled while the last ones were running.
func TestCancelAfterEverythingStarted(t *testing.T) {
	baseline := runtime.NumGoroutine()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	running := make(chan struct{}, 3)
	tasks := make([]Task, 3)
	for i := range tasks {
		tasks[i] = func(ctx context.Context) error {
			running <- struct{}{}
			<-ctx.Done()
			return nil
		}
	}
	out := start(ctx, 3, tasks)
	for i := 0; i < 3; i++ {
		wait(t, running, "all tasks to start")
	}
	cancel()
	if err := result(t, out); err != nil {
		t.Fatalf("Run returned %v, want nil: every task ran and returned nil", err)
	}
	settle(t, baseline)
}

// A task's own error wins over the context's.
func TestTaskErrorAfterCancel(t *testing.T) {
	baseline := runtime.NumGoroutine()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	mine := errors.New("mine")
	running := make(chan struct{}, 2)
	tasks := []Task{
		func(ctx context.Context) error {
			running <- struct{}{}
			<-ctx.Done()
			return mine
		},
		func(ctx context.Context) error { return nil },
		func(ctx context.Context) error { return nil },
	}
	out := start(ctx, 1, tasks)
	wait(t, running, "the first task")
	cancel()
	if err := result(t, out); err != mine {
		t.Fatalf("Run returned %v, want the task's error", err)
	}
	settle(t, baseline)
}

// Nothing is left running when the caller's context is never cancelled.
func TestNoLeakWithLongLivedContext(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	baseline := runtime.NumGoroutine()
	boom := errors.New("boom")
	for round := 0; round < 20; round++ {
		tasks := make([]Task, 10)
		for i := range tasks {
			i := i
			tasks[i] = func(ctx context.Context) error {
				if i == 3 && round%2 == 0 {
					return boom
				}
				return nil
			}
		}
		err := result(t, start(ctx, 3, tasks))
		if round%2 == 0 && err != boom {
			t.Fatalf("round %d: Run returned %v, want boom", round, err)
		}
		if round%2 == 1 && err != nil {
			t.Fatalf("round %d: Run returned %v, want nil", round, err)
		}
	}
	settle(t, baseline)
}
