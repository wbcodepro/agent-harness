package limiter

import "time"

// Limiter is a token bucket.
type Limiter struct {
	rate   float64
	burst  float64
	tokens float64
	last   time.Time
}

func New(rate float64, burst int, start time.Time) *Limiter {
	return &Limiter{rate: rate, burst: float64(burst), tokens: 0, last: start}
}

// Allow reports whether an event may happen at time now.
func (l *Limiter) Allow(now time.Time) bool {
	elapsed := now.Sub(l.last).Seconds()
	l.tokens += elapsed * l.rate
	if l.tokens >= 1 {
		l.tokens--
		l.last = now
		return true
	}
	return false
}
