package limiter

import (
	"testing"
	"time"
)

func TestBucket(t *testing.T) {
	start := time.Unix(1000, 0)
	l := New(2, 3, start)
	// Starts full: three at once, then no more.
	for i := 0; i < 3; i++ {
		if !l.Allow(start) {
			t.Fatalf("call %d at start should be allowed", i)
		}
	}
	if l.Allow(start) {
		t.Fatal("fourth call at start should be refused")
	}
	// Refused calls do not lose the time that has passed.
	if l.Allow(start.Add(200 * time.Millisecond)) {
		t.Fatal("0.4 tokens is not enough")
	}
	if !l.Allow(start.Add(500 * time.Millisecond)) {
		t.Fatal("one token after half a second")
	}
	// Never more than burst, however long it waits.
	later := start.Add(time.Hour)
	allowed := 0
	for i := 0; i < 10; i++ {
		if l.Allow(later) {
			allowed++
		}
	}
	if allowed != 3 {
		t.Fatalf("after a long wait %d were allowed, want 3", allowed)
	}
	// Time going backwards changes nothing.
	if l.Allow(start) {
		t.Fatal("no tokens are gained from a clock that went back")
	}
}
