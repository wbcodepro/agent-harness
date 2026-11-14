package wal

import (
	"bytes"
	"testing"
)

func TestPutGetReopen(t *testing.T) {
	dir := t.TempDir()
	l, err := Open(dir, Options{})
	if err != nil {
		t.Fatal(err)
	}
	if seq, err := l.Put("ab", []byte("xyz")); err != nil || seq != 1 {
		t.Fatalf("Put = %d, %v", seq, err)
	}
	if seq, err := l.Put("cd", []byte("1")); err != nil || seq != 2 {
		t.Fatalf("Put = %d, %v", seq, err)
	}
	if seq, err := l.Delete("cd"); err != nil || seq != 3 {
		t.Fatalf("Delete = %d, %v", seq, err)
	}
	if err := l.Close(); err != nil {
		t.Fatal(err)
	}
	l, err = Open(dir, Options{})
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	if v, ok := l.Get("ab"); !ok || !bytes.Equal(v, []byte("xyz")) {
		t.Fatalf("Get(ab) = %q, %v", v, ok)
	}
	if _, ok := l.Get("cd"); ok {
		t.Fatal("cd should be deleted")
	}
	if l.LastSeq() != 3 {
		t.Fatalf("LastSeq = %d", l.LastSeq())
	}
}
