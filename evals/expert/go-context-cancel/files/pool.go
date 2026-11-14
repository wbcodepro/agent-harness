// Package pool runs a list of tasks on a bounded number of goroutines.
package pool

import (
	"context"
	"errors"
)

// Task is one unit of work. It should return soon after ctx is cancelled.
type Task func(ctx context.Context) error

// ErrNoWorkers is returned by Run when workers is less than 1.
var ErrNoWorkers = errors.New("pool: workers must be at least 1")

// Run executes tasks on at most `workers` goroutines and returns the first
// error.
func Run(ctx context.Context, workers int, tasks []Task) error {
	if workers < 1 {
		return ErrNoWorkers
	}
	queue := make(chan Task)
	results := make(chan error)
	for i := 0; i < workers; i++ {
		go func() {
			for task := range queue {
				results <- task(ctx)
			}
		}()
	}
	go func() {
		for _, task := range tasks {
			queue <- task
		}
		close(queue)
	}()
	for range tasks {
		if err := <-results; err != nil {
			return err
		}
	}
	return nil
}
