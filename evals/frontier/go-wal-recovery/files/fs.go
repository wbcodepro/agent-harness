package wal

import (
	"os"
)

// FS is the file system the log works on. The path of the file name in
// the directory dir is dir + "/" + name.
type FS interface {
	// MkdirAll creates the directory, and its parents, if needed.
	MkdirAll(dir string) error
	// ReadDir returns the names of the files (not of subdirectories) in
	// dir, in no particular order.
	ReadDir(dir string) ([]string, error)
	// ReadFile returns the whole content of a file.
	ReadFile(path string) ([]byte, error)
	// Create creates a file, or empties an existing one, and opens it for
	// writing.
	Create(path string) (File, error)
	// OpenAppend opens an existing file for writing.
	OpenAppend(path string) (File, error)
	// Rename renames a file, replacing a file that already has the new name.
	Rename(oldpath, newpath string) error
	// Remove removes a file.
	Remove(path string) error
	// Truncate cuts a file back to size bytes.
	Truncate(path string, size int64) error
}

// File is a file opened for writing. Every Write appends at the end of
// the file. Close releases the file even when it returns an error.
type File interface {
	Write(p []byte) (n int, err error)
	Sync() error
	Close() error
}

// OSFS is the operating system's file system.
type OSFS struct{}

func (OSFS) MkdirAll(dir string) error { return os.MkdirAll(dir, 0o755) }

func (OSFS) ReadDir(dir string) ([]string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	var names []string
	for _, e := range entries {
		if !e.IsDir() {
			names = append(names, e.Name())
		}
	}
	return names, nil
}

func (OSFS) ReadFile(path string) ([]byte, error) { return os.ReadFile(path) }

func (OSFS) Create(path string) (File, error) {
	return os.OpenFile(path, os.O_CREATE|os.O_TRUNC|os.O_WRONLY|os.O_APPEND, 0o644)
}

func (OSFS) OpenAppend(path string) (File, error) {
	return os.OpenFile(path, os.O_WRONLY|os.O_APPEND, 0)
}

func (OSFS) Rename(oldpath, newpath string) error { return os.Rename(oldpath, newpath) }

func (OSFS) Remove(path string) error { return os.Remove(path) }

func (OSFS) Truncate(path string, size int64) error { return os.Truncate(path, size) }
