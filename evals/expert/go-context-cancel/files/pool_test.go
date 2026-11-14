package pool

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"
)

func TestRunsEveryTask(t *testing.T) {
	var ran atomic.Int64
	tasks := make([]Task, 20)
	for i := range tasks {
		tasks[i] = func(ctx context.Context) error {
			ran.Add(1)
			return nil
		}
	}
	if err := Run(context.Background(), 4, tasks); err != nil {
		t.Fatalf("Run returned %v", err)
	}
	if ran.Load() != 20 {
		t.Fatalf("ran %d tasks, want 20", ran.Load())
	}
}

func TestReturnsAnError(t *testing.T) {
	boom := errors.New("boom")
	tasks := []Task{
		func(ctx context.Context) error { return nil },
		func(ctx context.Context) error { return boom },
	}
	if err := Run(context.Background(), 1, tasks); !errors.Is(err, boom) {
		t.Fatalf("Run returned %v, want boom", err)
	}
}

func TestNoWorkers(t *testing.T) {
	if err := Run(context.Background(), 0, nil); !errors.Is(err, ErrNoWorkers) {
		t.Fatalf("Run returned %v, want ErrNoWorkers", err)
	}
}
