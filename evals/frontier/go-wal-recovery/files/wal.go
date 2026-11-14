// Package wal is a small durable key-value log. SPEC.md is its contract.
package wal

import (
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"
	"sort"
	"strings"
)

const (
	DefaultSegmentSize = 4 << 20
	HeaderSize         = 16
	MaxKeySize         = 65535
	MaxValueSize       = 1 << 20
	MaxPayload         = 11 + MaxKeySize + MaxValueSize
)

// Kind says what a record does.
type Kind uint8

const (
	KindPut    Kind = 1
	KindDelete Kind = 2
)

// BatchFlag is bit 7 of a record's kind byte: the batch goes on after this
// record.
const BatchFlag = 0x80

// Record is one change, as Apply takes it.
type Record struct {
	Seq   uint64
	Kind  Kind
	Key   string
	Value []byte
}

// Op is one change of a batch.
type Op struct {
	Kind  Kind
	Key   string
	Value []byte
}

// Options configure Open.
type Options struct {
	FS          FS    // nil means OSFS{}
	SegmentSize int64 // 0 means DefaultSegmentSize
}

var (
	ErrKeyTooLarge    = errors.New("wal: key too large")
	ErrValueTooLarge  = errors.New("wal: value too large")
	ErrInvalidRecord  = errors.New("wal: invalid record")
	ErrInvalidOptions = errors.New("wal: invalid options")
	ErrEmptyBatch     = errors.New("wal: empty batch")
	ErrClosed         = errors.New("wal: log is closed")
	ErrPoisoned       = errors.New("wal: log is poisoned by an earlier error")
	ErrCorrupt        = errors.New("wal: corrupt segment")
)

// CorruptError reports a corrupt segment: its file name and the offset of
// the problem.
type CorruptError struct {
	Segment string
	Offset  int64
}

func (e *CorruptError) Error() string {
	return fmt.Sprintf("wal: corrupt segment %s at offset %d", e.Segment, e.Offset)
}

func (e *CorruptError) Is(target error) bool { return target == ErrCorrupt }

// Log is an open log. It is used from one goroutine at a time.
type Log struct {
	fs     FS
	dir    string
	active File
	data   map[string][]byte
	last   uint64
	closed bool
}

func Open(dir string, opts Options) (*Log, error) {
	fsys := opts.FS
	if fsys == nil {
		fsys = OSFS{}
	}
	if err := fsys.MkdirAll(dir); err != nil {
		return nil, err
	}
	names, err := fsys.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	var segs []string
	for _, n := range names {
		if strings.HasSuffix(n, ".seg") {
			segs = append(segs, n)
		}
	}
	sort.Strings(segs)
	l := &Log{fs: fsys, dir: dir, data: map[string][]byte{}}
	for _, n := range segs {
		b, err := fsys.ReadFile(dir + "/" + n)
		if err != nil {
			return nil, err
		}
		if len(b) < HeaderSize || string(b[:4]) != "WSEG" {
			return nil, &CorruptError{Segment: n}
		}
		p := HeaderSize
		for p < len(b) {
			if len(b)-p < 8 {
				return nil, &CorruptError{Segment: n, Offset: int64(p)}
			}
			size := int(binary.LittleEndian.Uint32(b[p:]))
			sum := binary.LittleEndian.Uint32(b[p+4:])
			if size < 11 || len(b)-p-8 < size {
				return nil, &CorruptError{Segment: n, Offset: int64(p)}
			}
			payload := b[p+8 : p+8+size]
			if crc32.ChecksumIEEE(payload) != sum {
				return nil, &CorruptError{Segment: n, Offset: int64(p)}
			}
			seq := binary.LittleEndian.Uint64(payload)
			kind := Kind(payload[8])
			k := int(binary.LittleEndian.Uint16(payload[9:]))
			key := string(payload[11 : 11+k])
			if kind == KindPut {
				l.data[key] = append([]byte(nil), payload[11+k:]...)
			} else {
				delete(l.data, key)
			}
			l.last = seq
			p += 8 + size
		}
	}
	var name string
	if len(segs) == 0 {
		name = "00000001.seg"
		f, err := fsys.Create(dir + "/" + name)
		if err != nil {
			return nil, err
		}
		hdr := make([]byte, HeaderSize)
		copy(hdr, "WSEG")
		binary.LittleEndian.PutUint16(hdr[4:], 1)
		if _, err := f.Write(hdr); err != nil {
			f.Close()
			return nil, err
		}
		if err := f.Sync(); err != nil {
			f.Close()
			return nil, err
		}
		l.active = f
		return l, nil
	}
	name = segs[len(segs)-1]
	f, err := fsys.OpenAppend(dir + "/" + name)
	if err != nil {
		return nil, err
	}
	l.active = f
	return l, nil
}

func (l *Log) write(seq uint64, kind Kind, key string, value []byte) error {
	if l.closed {
		return ErrClosed
	}
	if len(key) > MaxKeySize {
		return ErrKeyTooLarge
	}
	if len(value) > MaxValueSize {
		return ErrValueTooLarge
	}
	payload := make([]byte, 11+len(key)+len(value))
	binary.LittleEndian.PutUint64(payload, seq)
	payload[8] = byte(kind)
	binary.LittleEndian.PutUint16(payload[9:], uint16(len(key)))
	copy(payload[11:], key)
	copy(payload[11+len(key):], value)
	rec := make([]byte, 8, 8+len(payload))
	binary.LittleEndian.PutUint32(rec, uint32(len(payload)))
	binary.LittleEndian.PutUint32(rec[4:], crc32.ChecksumIEEE(payload))
	rec = append(rec, payload...)
	if _, err := l.active.Write(rec); err != nil {
		return err
	}
	if err := l.active.Sync(); err != nil {
		return err
	}
	if kind == KindPut {
		l.data[key] = append([]byte(nil), value...)
	} else {
		delete(l.data, key)
	}
	l.last = seq
	return nil
}

func (l *Log) Put(key string, value []byte) (uint64, error) {
	seq := l.last + 1
	if err := l.write(seq, KindPut, key, value); err != nil {
		return 0, err
	}
	return seq, nil
}

func (l *Log) Delete(key string) (uint64, error) {
	seq := l.last + 1
	if err := l.write(seq, KindDelete, key, nil); err != nil {
		return 0, err
	}
	return seq, nil
}

func (l *Log) Apply(rec Record) (bool, error) {
	if rec.Seq <= l.last {
		return false, nil
	}
	if err := l.write(rec.Seq, rec.Kind, rec.Key, rec.Value); err != nil {
		return false, err
	}
	return true, nil
}

func (l *Log) Batch(ops []Op) (uint64, error) {
	return 0, errors.New("wal: Batch is not implemented")
}

func (l *Log) Get(key string) ([]byte, bool) {
	v, ok := l.data[key]
	return v, ok
}

func (l *Log) Keys() []string {
	var keys []string
	for k := range l.data {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func (l *Log) LastSeq() uint64 { return l.last }

func (l *Log) Compact() error {
	return errors.New("wal: Compact is not implemented")
}

func (l *Log) Close() error {
	if l.closed {
		return nil
	}
	l.closed = true
	return l.active.Close()
}
