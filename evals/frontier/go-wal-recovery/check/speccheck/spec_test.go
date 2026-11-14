// Package speccheck checks package wal against SPEC.md, rule by rule.
package speccheck

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"wal"
)

// ---------------------------------------------------------------------------
// An in-memory file system that counts calls, can fail one of them, records
// what is durable after every change, and refuses to read foreign files.

var errBoom = errors.New("injected file system failure")
var errForbidden = errors.New("read of a file the log must not read")

type mfile struct {
	data   []byte
	synced int
}

type image struct {
	tag   int
	files map[string]mfile
	dirs  map[string]bool
}

type memFS struct {
	files    map[string]*mfile
	dirs     map[string]bool
	handles  []*mhandle
	ops      int
	failAt   int
	failed   bool
	forbid   map[string]bool
	badReads []string
	record   bool
	tag      int
	images   []image
	trace    []string
}

type mhandle struct {
	m      *memFS
	name   string
	f      *mfile
	closed bool
}

func newMem() *memFS {
	return &memFS{files: map[string]*mfile{}, dirs: map[string]bool{}, forbid: map[string]bool{}}
}

func (m *memFS) step() error {
	m.ops++
	if m.failAt != 0 && m.ops == m.failAt {
		m.failed = true
		return errBoom
	}
	return nil
}

func base(p string) string { return p[strings.LastIndex(p, "/")+1:] }

func (m *memFS) note(s string) { m.trace = append(m.trace, s) }

func (m *memFS) snap() {
	if !m.record {
		return
	}
	img := image{tag: m.tag, files: map[string]mfile{}, dirs: map[string]bool{}}
	for p, f := range m.files {
		img.files[p] = mfile{data: append([]byte(nil), f.data...), synced: f.synced}
	}
	for d := range m.dirs {
		img.dirs[d] = true
	}
	m.images = append(m.images, img)
}

// fs returns the directory as a crash leaves it: variant 0 loses all
// unsynced data, 1 keeps it all, 2 keeps half of it.
func (img image) fs(variant int) *memFS {
	m := newMem()
	for d := range img.dirs {
		m.dirs[d] = true
	}
	for p, f := range img.files {
		n := f.synced
		switch variant {
		case 1:
			n = len(f.data)
		case 2:
			n = f.synced + (len(f.data)-f.synced+1)/2
		}
		m.files[p] = &mfile{data: append([]byte(nil), f.data[:n]...), synced: n}
	}
	return m
}

func (m *memFS) openCount() int {
	n := 0
	for _, h := range m.handles {
		if !h.closed {
			n++
		}
	}
	return n
}

func (m *memFS) MkdirAll(dir string) error {
	if err := m.step(); err != nil {
		return err
	}
	m.dirs[dir] = true
	m.snap()
	return nil
}

func (m *memFS) ReadDir(dir string) ([]string, error) {
	if err := m.step(); err != nil {
		return nil, err
	}
	if !m.dirs[dir] {
		return nil, os.ErrNotExist
	}
	var names []string
	for p := range m.files {
		if strings.HasPrefix(p, dir+"/") && !strings.Contains(p[len(dir)+1:], "/") {
			names = append(names, p[len(dir)+1:])
		}
	}
	// "in no particular order": hand them out in descending order.
	sort.Sort(sort.Reverse(sort.StringSlice(names)))
	return names, nil
}

func (m *memFS) ReadFile(path string) ([]byte, error) {
	if err := m.step(); err != nil {
		return nil, err
	}
	if m.forbid[path] {
		m.badReads = append(m.badReads, base(path))
		return nil, errForbidden
	}
	f, ok := m.files[path]
	if !ok {
		return nil, os.ErrNotExist
	}
	return append([]byte(nil), f.data...), nil
}

func (m *memFS) handle(path string, f *mfile) *mhandle {
	h := &mhandle{m: m, name: base(path), f: f}
	m.handles = append(m.handles, h)
	return h
}

func (m *memFS) Create(path string) (wal.File, error) {
	if err := m.step(); err != nil {
		return nil, err
	}
	f, ok := m.files[path]
	if ok {
		f.data, f.synced = nil, 0
	} else {
		f = &mfile{}
		m.files[path] = f
	}
	m.note("create " + base(path))
	m.snap()
	return m.handle(path, f), nil
}

func (m *memFS) OpenAppend(path string) (wal.File, error) {
	if err := m.step(); err != nil {
		return nil, err
	}
	f, ok := m.files[path]
	if !ok {
		return nil, os.ErrNotExist
	}
	m.note("openappend " + base(path))
	return m.handle(path, f), nil
}

func (m *memFS) Rename(oldpath, newpath string) error {
	if err := m.step(); err != nil {
		return err
	}
	f, ok := m.files[oldpath]
	if !ok {
		return os.ErrNotExist
	}
	delete(m.files, oldpath)
	m.files[newpath] = f
	m.note("rename " + base(oldpath) + " " + base(newpath))
	m.snap()
	return nil
}

func (m *memFS) Remove(path string) error {
	if err := m.step(); err != nil {
		return err
	}
	if _, ok := m.files[path]; !ok {
		return os.ErrNotExist
	}
	delete(m.files, path)
	m.note("remove " + base(path))
	m.snap()
	return nil
}

func (m *memFS) Truncate(path string, size int64) error {
	if err := m.step(); err != nil {
		return err
	}
	f, ok := m.files[path]
	if !ok {
		return os.ErrNotExist
	}
	if size < 0 || size > int64(len(f.data)) {
		return errors.New("truncate beyond the end of the file")
	}
	f.data = f.data[:size]
	if f.synced > int(size) {
		f.synced = int(size)
	}
	m.note(fmt.Sprintf("truncate %s %d", base(path), size))
	m.snap()
	return nil
}

func (h *mhandle) Write(p []byte) (int, error) {
	if err := h.m.step(); err != nil {
		return 0, err
	}
	if h.closed {
		return 0, errors.New("write to a closed file")
	}
	h.f.data = append(h.f.data, p...)
	if n := len(h.m.trace); n > 0 && strings.HasPrefix(h.m.trace[n-1], "write "+h.name+" ") {
		var size int
		fmt.Sscanf(h.m.trace[n-1][len("write "+h.name+" "):], "%d", &size)
		h.m.trace[n-1] = fmt.Sprintf("write %s %d", h.name, size+len(p))
	} else {
		h.m.note(fmt.Sprintf("write %s %d", h.name, len(p)))
	}
	h.m.snap()
	return len(p), nil
}

func (h *mhandle) Sync() error {
	if err := h.m.step(); err != nil {
		return err
	}
	if h.closed {
		return errors.New("sync of a closed file")
	}
	h.f.synced = len(h.f.data)
	h.m.note("sync " + h.name)
	h.m.snap()
	return nil
}

func (h *mhandle) Close() error {
	err := h.m.step()
	if h.closed {
		return errors.New("close of a closed file")
	}
	h.closed = true
	h.m.note("close " + h.name)
	return err
}

// files returns the directory db as name -> content.
func (m *memFS) dir() map[string]string {
	out := map[string]string{}
	for p, f := range m.files {
		if strings.HasPrefix(p, "db/") {
			out[p[3:]] = string(f.data)
		}
	}
	return out
}

func memWith(files map[string][]byte) *memFS {
	m := newMem()
	m.dirs["db"] = true
	for n, b := range files {
		m.files["db/"+n] = &mfile{data: append([]byte(nil), b...), synced: len(b)}
	}
	return m
}

// ---------------------------------------------------------------------------
// The file format, written independently of the package.

func hdr(snap bool, seq uint64) []byte {
	h := make([]byte, 16)
	copy(h, "WSEG")
	binary.LittleEndian.PutUint16(h[4:], 1)
	if snap {
		binary.LittleEndian.PutUint16(h[6:], 1)
	}
	binary.LittleEndian.PutUint64(h[8:], seq)
	return h
}

func rawHdr(magic string, version, flags uint16, seq uint64) []byte {
	h := make([]byte, 16)
	copy(h, magic)
	binary.LittleEndian.PutUint16(h[4:], version)
	binary.LittleEndian.PutUint16(h[6:], flags)
	binary.LittleEndian.PutUint64(h[8:], seq)
	return h
}

func payload(seq uint64, kind byte, klen int, key, val string) []byte {
	p := make([]byte, 11, 11+len(key)+len(val))
	binary.LittleEndian.PutUint64(p, seq)
	p[8] = kind
	binary.LittleEndian.PutUint16(p[9:], uint16(klen))
	p = append(p, key...)
	return append(p, val...)
}

func frame(p []byte) []byte {
	r := make([]byte, 8, 8+len(p))
	binary.LittleEndian.PutUint32(r, uint32(len(p)))
	binary.LittleEndian.PutUint32(r[4:], crc32.ChecksumIEEE(p))
	return append(r, p...)
}

func put(seq uint64, key, val string) []byte { return frame(payload(seq, 1, len(key), key, val)) }
func del(seq uint64, key string) []byte      { return frame(payload(seq, 2, len(key), key, "")) }

func cat(parts ...[]byte) []byte {
	var b []byte
	for _, p := range parts {
		b = append(b, p...)
	}
	return b
}

func seg(n int) string { return fmt.Sprintf("%08d.seg", n) }

// hdrL is the header of a segment that is not a snapshot, with its link.
func hdrL(link uint32) []byte {
	h := hdr(false, 0)
	binary.LittleEndian.PutUint32(h[8:], link)
	return h
}

// crcOf is the CRC field of an encoded record (or of the last of several).
func crcOf(recs ...[]byte) uint32 {
	r := recs[len(recs)-1]
	return binary.LittleEndian.Uint32(r[4:])
}

// chained returns a copy of a directory in which every live segment that
// is not a snapshot and has a valid header gets the link rules 39 to 41
// require. Only the link field is touched.
func chained(files map[string][]byte) map[string][]byte {
	out := map[string][]byte{}
	var nums []int
	for n, b := range files {
		out[n] = append([]byte(nil), b...)
		var k int
		if len(n) == 12 && strings.HasSuffix(n, ".seg") && strings.Trim(n[:8], "0123456789") == "" {
			fmt.Sscanf(n[:8], "%d", &k)
			if k >= 1 {
				nums = append(nums, k)
			}
		}
	}
	sort.Ints(nums)
	valid := func(b []byte) (ok, snap bool) {
		if len(b) < 16 || string(b[:4]) != "WSEG" || binary.LittleEndian.Uint16(b[4:]) != 1 {
			return false, false
		}
		f := binary.LittleEndian.Uint16(b[6:])
		if f&^1 != 0 {
			return false, false
		}
		if f == 0 && binary.LittleEndian.Uint32(b[12:]) != 0 {
			return false, false
		}
		return true, f == 1
	}
	first := 0
	for i := len(nums) - 1; i >= 0; i-- {
		if ok, snap := valid(out[seg(nums[i])]); ok && snap {
			first = i
			break
		}
	}
	var prev uint32
	for _, k := range nums[first:] {
		b := out[seg(k)]
		ok, snap := valid(b)
		if !ok {
			prev = 0
			continue
		}
		chain := uint32(0)
		if !snap {
			binary.LittleEndian.PutUint32(b[8:], prev)
			chain = prev
		}
		for p := 16; p+8 <= len(b); {
			n := int(binary.LittleEndian.Uint32(b[p:]))
			if n < 11 || n > wal.MaxPayload || p+8+n > len(b) || crc32.ChecksumIEEE(b[p+8:p+8+n]) != binary.LittleEndian.Uint32(b[p+4:]) {
				break
			}
			chain = binary.LittleEndian.Uint32(b[p+4:])
			p += 8 + n
		}
		prev = chain
	}
	return out
}

// ---------------------------------------------------------------------------
// The visible state of a log.

type st struct {
	kv   map[string]string
	last uint64
}

func mk(last uint64, kv ...string) st {
	s := st{kv: map[string]string{}, last: last}
	for i := 0; i+1 < len(kv); i += 2 {
		s.kv[kv[i]] = kv[i+1]
	}
	return s
}

func (s st) clone() st {
	c := st{kv: map[string]string{}, last: s.last}
	for k, v := range s.kv {
		c.kv[k] = v
	}
	return c
}

func (s st) String() string {
	keys := make([]string, 0, len(s.kv))
	for k := range s.kv {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var b strings.Builder
	fmt.Fprintf(&b, "{LastSeq %d", s.last)
	for _, k := range keys {
		v := s.kv[k]
		if len(v) > 24 {
			v = v[:24] + "..."
		}
		fmt.Fprintf(&b, " %q=%q", k, v)
	}
	return b.String() + "}"
}

func same(a, b st) bool {
	if a.last != b.last || len(a.kv) != len(b.kv) {
		return false
	}
	for k, v := range a.kv {
		if w, ok := b.kv[k]; !ok || w != v {
			return false
		}
	}
	return true
}

func observe(l *wal.Log) st {
	s := st{kv: map[string]string{}, last: l.LastSeq()}
	for _, k := range l.Keys() {
		v, ok := l.Get(k)
		if !ok {
			s.kv[k] = "<Keys lists it but Get says it does not exist>"
			continue
		}
		s.kv[k] = string(v)
	}
	return s
}

func mustOpen(t *testing.T, m *memFS, size int64) *wal.Log {
	t.Helper()
	l, err := wal.Open("db", wal.Options{FS: m, SegmentSize: size})
	if err != nil {
		t.Fatalf("Open: unexpected error %v", err)
	}
	return l
}

func wantState(t *testing.T, l *wal.Log, want st, rule string) {
	t.Helper()
	if got := observe(l); !same(got, want) {
		t.Fatalf("%s: state is %v, want %v", rule, got, want)
	}
}

func dirDiff(got, want map[string]string) string {
	var out []string
	for n, w := range want {
		g, ok := got[n]
		if !ok {
			out = append(out, fmt.Sprintf("%s is missing", n))
		} else if g != w {
			out = append(out, fmt.Sprintf("%s has %d bytes %x, want %d bytes %x", n, len(g), short(g), len(w), short(w)))
		}
	}
	for n := range got {
		if _, ok := want[n]; !ok {
			out = append(out, fmt.Sprintf("%s should not exist", n))
		}
	}
	sort.Strings(out)
	return strings.Join(out, "; ")
}

func short(s string) string {
	if len(s) > 80 {
		return s[:80]
	}
	return s
}

func strs(files map[string][]byte) map[string]string {
	out := map[string]string{}
	for n, b := range files {
		out[n] = string(b)
	}
	return out
}

// ---------------------------------------------------------------------------
// Rules 4, 6, 10, 26 and the example.

func TestExampleBytes(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	if !m.dirs["db"] {
		t.Fatal("rule 16: Open must call MkdirAll(dir)")
	}
	want := map[string]string{seg(1): string(hdr(false, 0))}
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rule 26: a new log has one empty segment: %s", d)
	}
	if seq, err := l.Put("ab", []byte("xyz")); err != nil || seq != 1 {
		t.Fatalf("rule 6: Put = %d, %v; want 1, nil", seq, err)
	}
	r1 := put(1, "ab", "xyz")
	if hex := fmt.Sprintf("%x", r1); hex != "1000000073c954380100000000000000010200616278797a" {
		t.Fatalf("checker self-test failed: %s", hex)
	}
	want[seg(1)] = string(cat(hdr(false, 0), r1))
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rule 4: %s", d)
	}
	if seq, err := l.Delete("ab"); err != nil || seq != 2 {
		t.Fatalf("rule 6: Delete = %d, %v; want 2, nil", seq, err)
	}
	want[seg(1)] = string(cat(hdr(false, 0), r1, del(2, "ab")))
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rule 4: %s", d)
	}
	if err := l.Compact(); err != nil {
		t.Fatalf("Compact: %v", err)
	}
	want = map[string]string{seg(2): string(hdr(true, 2)), seg(3): string(hdr(false, 0))}
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rules 28-29: after Compact: %s", d)
	}
	wantState(t, l, mk(2), "rule 29")
	if err := l.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestRecordsAndSeqs(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	type op struct {
		key, val string
		del      bool
	}
	ops := []op{{"k1", "v1", false}, {"", "", false}, {"k1", "", true}, {"nope", "", true}, {"k2", strings.Repeat("z", 300), false}, {"k1", "again", false}}
	file := hdr(false, 0)
	for i, o := range ops {
		var seq uint64
		var err error
		if o.del {
			seq, err = l.Delete(o.key)
			file = cat(file, del(uint64(i+1), o.key))
		} else {
			seq, err = l.Put(o.key, []byte(o.val))
			file = cat(file, put(uint64(i+1), o.key, o.val))
		}
		if err != nil || seq != uint64(i+1) {
			t.Fatalf("rule 6: call %d returned %d, %v; want %d, nil", i+1, seq, err, i+1)
		}
		if l.LastSeq() != uint64(i+1) {
			t.Fatalf("rule 6: LastSeq = %d, want %d", l.LastSeq(), i+1)
		}
	}
	if d := dirDiff(m.dir(), map[string]string{seg(1): string(file)}); d != "" {
		t.Fatalf("rules 4, 6 (Delete of a missing key appends a record too): %s", d)
	}
	wantState(t, l, mk(6, "", "", "k1", "again", "k2", strings.Repeat("z", 300)), "rule 12")
	l.Close()
	l = mustOpen(t, m, 0)
	wantState(t, l, mk(6, "", "", "k1", "again", "k2", strings.Repeat("z", 300)), "rules 24, 26 (reopen)")
	if seq, err := l.Put("k3", []byte("x")); err != nil || seq != 7 {
		t.Fatalf("rule 6: Put after reopen = %d, %v; want 7", seq, err)
	}
	file = cat(file, put(7, "k3", "x"))
	if d := dirDiff(m.dir(), map[string]string{seg(1): string(file)}); d != "" {
		t.Fatalf("rule 26: the reopened log appends to its highest segment: %s", d)
	}
	l.Close()
}

// ---------------------------------------------------------------------------
// Rules 7, 8, 11, 12.

func TestApply(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	recs := []wal.Record{
		{Seq: 5, Kind: wal.KindPut, Key: "a", Value: []byte("1")},
		{Seq: 9, Kind: wal.KindPut, Key: "b", Value: []byte("2")},
		{Seq: 10, Kind: wal.KindDelete, Key: "zz"},
		{Seq: 12, Kind: wal.KindDelete, Key: "a"},
	}
	for _, r := range recs {
		ok, err := l.Apply(r)
		if err != nil || !ok {
			t.Fatalf("rule 7: Apply(seq %d) = %v, %v; want true, nil", r.Seq, ok, err)
		}
	}
	want := mk(12, "b", "2")
	wantState(t, l, want, "rule 7")
	before := m.dir()
	ops := m.ops
	for _, r := range append(recs, wal.Record{Seq: 11, Kind: wal.KindPut, Key: "c", Value: []byte("3")}, wal.Record{Seq: 0, Kind: wal.KindPut, Key: "d"}) {
		ok, err := l.Apply(r)
		if err != nil || ok {
			t.Fatalf("rule 7: stale Apply(seq %d) = %v, %v; want false, nil", r.Seq, ok, err)
		}
	}
	if m.ops != ops || dirDiff(m.dir(), before) != "" {
		t.Fatal("rule 7: a stale record must write nothing")
	}
	wantState(t, l, want, "rule 7 (stale records change nothing)")
	if seq, err := l.Put("c", []byte("x")); err != nil || seq != 13 {
		t.Fatalf("rule 6: Put after Apply(12) = %d, %v; want 13", seq, err)
	}
	l.Close()
	l = mustOpen(t, m, 0)
	wantState(t, l, mk(13, "b", "2", "c", "x"), "rule 7 (reopen)")
	for _, r := range recs {
		if ok, err := l.Apply(r); err != nil || ok {
			t.Fatalf("rule 7: replaying seq %d after reopen = %v, %v; want false, nil", r.Seq, ok, err)
		}
	}
	l.Close()
}

func TestArgumentErrors(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	l.Put("keep", []byte("me"))
	bigKey := strings.Repeat("k", wal.MaxKeySize+1)
	bigVal := make([]byte, wal.MaxValueSize+1)
	type tc struct {
		name string
		do   func() error
		want error
	}
	apply := func(r wal.Record) func() error {
		return func() error { _, err := l.Apply(r); return err }
	}
	cases := []tc{
		{"Put with a key of 65536 bytes", func() error { _, err := l.Put(bigKey, nil); return err }, wal.ErrKeyTooLarge},
		{"Put with a too long key and value", func() error { _, err := l.Put(bigKey, bigVal); return err }, wal.ErrKeyTooLarge},
		{"Put with a value of MaxValueSize+1 bytes", func() error { _, err := l.Put("k", bigVal); return err }, wal.ErrValueTooLarge},
		{"Delete with a key of 65536 bytes", func() error { _, err := l.Delete(bigKey); return err }, wal.ErrKeyTooLarge},
		{"Apply with kind 0", apply(wal.Record{Seq: 50, Kind: 0, Key: "k"}), wal.ErrInvalidRecord},
		{"Apply with kind 3", apply(wal.Record{Seq: 50, Kind: 3, Key: "k"}), wal.ErrInvalidRecord},
		{"Apply of a delete with a value", apply(wal.Record{Seq: 50, Kind: wal.KindDelete, Key: "k", Value: []byte("v")}), wal.ErrInvalidRecord},
		{"Apply with a long key and kind 9", apply(wal.Record{Seq: 50, Kind: 9, Key: bigKey}), wal.ErrKeyTooLarge},
		{"Apply of a delete with a long value", apply(wal.Record{Seq: 50, Kind: wal.KindDelete, Key: "k", Value: bigVal}), wal.ErrValueTooLarge},
		{"Apply of a stale record with kind 7", apply(wal.Record{Seq: 1, Kind: 7, Key: "k"}), wal.ErrInvalidRecord},
		{"Apply of a stale record with a long key", apply(wal.Record{Seq: 0, Kind: wal.KindPut, Key: bigKey}), wal.ErrKeyTooLarge},
	}
	for _, c := range cases {
		ops := m.ops
		err := c.do()
		if !errors.Is(err, c.want) {
			t.Fatalf("rule 8: %s returned %v, want %v", c.name, err, c.want)
		}
		if m.ops != ops {
			t.Fatalf("rule 8: %s must not call the file system", c.name)
		}
		wantState(t, l, mk(1, "keep", "me"), "rule 8 ("+c.name+" changes nothing)")
	}
	maxKey := strings.Repeat("q", wal.MaxKeySize)
	maxVal := bytes.Repeat([]byte{7}, wal.MaxValueSize)
	if seq, err := l.Put(maxKey, maxVal); err != nil || seq != 2 {
		t.Fatalf("rules 5, 8: a key of 65535 bytes with a value of MaxValueSize bytes is fine; Put = %d, %v", seq, err)
	}
	l.Close()
	l = mustOpen(t, m, 0)
	if v, ok := l.Get(maxKey); !ok || !bytes.Equal(v, maxVal) {
		t.Fatal("rules 5, 24: the largest record must survive a reopen")
	}
	l.Close()
}

func TestCopiesAndGet(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	v := []byte("hello")
	l.Put("a", v)
	v[0] = 'J'
	if g, _ := l.Get("a"); string(g) != "hello" {
		t.Fatalf("rule 11: Put must store a copy, Get = %q", g)
	}
	g, _ := l.Get("a")
	g[0] = 'Y'
	if g2, _ := l.Get("a"); string(g2) != "hello" {
		t.Fatalf("rule 11: Get must return a copy, Get = %q", g2)
	}
	av := []byte("applied")
	l.Apply(wal.Record{Seq: 7, Kind: wal.KindPut, Key: "b", Value: av})
	av[0] = 'X'
	if g, _ := l.Get("b"); string(g) != "applied" {
		t.Fatalf("rule 11: Apply must store a copy, Get = %q", g)
	}
	l.Put("empty", []byte{})
	l.Put("nilval", nil)
	if g, ok := l.Get("empty"); !ok || len(g) != 0 {
		t.Fatalf("rule 12: a key put with an empty value exists: Get = %q, %v", g, ok)
	}
	if g, ok := l.Get("nilval"); !ok || len(g) != 0 {
		t.Fatalf("rule 12: a key put with a nil value exists: Get = %q, %v", g, ok)
	}
	l.Delete("a")
	if g, ok := l.Get("a"); ok || g != nil {
		t.Fatalf("rule 12: Get of a deleted key = %q, %v; want nil, false", g, ok)
	}
	if g, ok := l.Get("never"); ok || g != nil {
		t.Fatalf("rule 12: Get of a missing key = %q, %v; want nil, false", g, ok)
	}
	for _, k := range []string{"b2", "B", "a", "\x00", "é", "ab", "Z9", ""} {
		l.Put(k, []byte("v"))
	}
	keys := l.Keys()
	want := []string{"", "\x00", "B", "Z9", "a", "ab", "b", "b2", "empty", "nilval", "é"}
	if fmt.Sprint(keys) != fmt.Sprint(want) {
		t.Fatalf("rule 12: Keys = %q, want %q", keys, want)
	}
	l.Close()
	l = mustOpen(t, m, 0)
	if fmt.Sprint(l.Keys()) != fmt.Sprint(want) {
		t.Fatalf("rule 12: Keys after reopen = %q, want %q", l.Keys(), want)
	}
	if g, ok := l.Get("empty"); !ok || len(g) != 0 {
		t.Fatal("rule 12: an empty value must survive a reopen")
	}
	l.Close()
}

// ---------------------------------------------------------------------------
// Rules 13, 14, 15, 16.

func TestClosed(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	l.Put("a", []byte("1"))
	if m.openCount() != 1 {
		t.Fatalf("rule 15: an open log holds exactly one open File, it holds %d", m.openCount())
	}
	if err := l.Close(); err != nil {
		t.Fatalf("rule 14: Close = %v", err)
	}
	if m.openCount() != 0 {
		t.Fatal("rule 15: Close must close the active segment")
	}
	ops := m.ops
	big := strings.Repeat("k", wal.MaxKeySize+1)
	errs := map[string]error{}
	_, errs["Put"] = l.Put("b", []byte("2"))
	_, errs["Put with a long key"] = l.Put(big, nil)
	_, errs["Delete"] = l.Delete("a")
	_, errs["Apply"] = l.Apply(wal.Record{Seq: 9, Kind: wal.KindPut, Key: "c"})
	_, errs["Apply with kind 5"] = l.Apply(wal.Record{Seq: 9, Kind: 5, Key: "c"})
	_, errs["Apply of a stale record"] = l.Apply(wal.Record{Seq: 1, Kind: wal.KindPut, Key: "c"})
	errs["Compact"] = l.Compact()
	for name, err := range errs {
		if !errors.Is(err, wal.ErrClosed) {
			t.Fatalf("rule 14: %s after Close returned %v, want ErrClosed", name, err)
		}
	}
	if m.ops != ops {
		t.Fatal("rule 14: calls after Close must not touch the file system")
	}
	wantState(t, l, mk(1, "a", "1"), "rule 14 (Get, Keys, LastSeq after Close)")
	if err := l.Close(); err != nil {
		t.Fatalf("rule 14: a second Close = %v, want nil", err)
	}
}

func TestPoisoned(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	l.Put("a", []byte("1"))
	m.failAt = m.ops + 2 // the Sync of the next Put
	_, err := l.Put("b", []byte("2"))
	if !m.failed {
		t.Fatal("rule 10: Put must call Write and then Sync")
	}
	if !errors.Is(err, errBoom) {
		t.Fatalf("rule 13: Put returned %v, want an error wrapping the file system's error", err)
	}
	wantState(t, l, mk(1, "a", "1"), "rule 13 (state as before the failed call)")
	ops := m.ops
	big := strings.Repeat("k", wal.MaxKeySize+1)
	errs := map[string]error{}
	_, errs["Put"] = l.Put("c", []byte("3"))
	_, errs["Put with a long key"] = l.Put(big, nil)
	_, errs["Delete"] = l.Delete("a")
	_, errs["Apply"] = l.Apply(wal.Record{Seq: 9, Kind: wal.KindPut, Key: "c"})
	_, errs["Apply with kind 5"] = l.Apply(wal.Record{Seq: 9, Kind: 5, Key: "c"})
	_, errs["Apply of a stale record"] = l.Apply(wal.Record{Seq: 1, Kind: wal.KindPut, Key: "c"})
	errs["Compact"] = l.Compact()
	for name, err := range errs {
		if !errors.Is(err, wal.ErrPoisoned) {
			t.Fatalf("rule 13: %s on a poisoned log returned %v, want ErrPoisoned", name, err)
		}
	}
	if m.ops != ops {
		t.Fatal("rule 13: a poisoned log must not call the file system")
	}
	if m.openCount() > 1 {
		t.Fatal("rule 15: a poisoned log holds at most one open File")
	}
	if err := l.Close(); err != nil {
		t.Fatalf("rule 13: Close of a poisoned log = %v", err)
	}
	if m.openCount() != 0 {
		t.Fatal("rule 15: no File may be left open after Close")
	}
	_, err = l.Put("d", nil)
	if !errors.Is(err, wal.ErrClosed) {
		t.Fatalf("rule 14: Put on a closed poisoned log returned %v, want ErrClosed", err)
	}
	if err := l.Compact(); !errors.Is(err, wal.ErrClosed) {
		t.Fatalf("rule 14: Compact on a closed poisoned log returned %v, want ErrClosed", err)
	}
}

func TestInvalidOptions(t *testing.T) {
	m := newMem()
	_, err := wal.Open("db", wal.Options{FS: m, SegmentSize: -1})
	if !errors.Is(err, wal.ErrInvalidOptions) {
		t.Fatalf("rule 16: Open with SegmentSize -1 returned %v, want ErrInvalidOptions", err)
	}
	if m.ops != 0 {
		t.Fatal("rule 16: Open with a negative SegmentSize must not call the file system")
	}
}

// ---------------------------------------------------------------------------
// Rule 9 and the order of rule 10.

func TestRotation(t *testing.T) {
	r := int64(len(put(1, "k1", "0123456789")))
	m := newMem()
	l := mustOpen(t, m, 16+2*r)
	l.Put("k1", []byte("0123456789"))
	l.Put("k2", []byte("0123456789"))
	if d := dirDiff(m.dir(), map[string]string{seg(1): string(cat(hdr(false, 0), put(1, "k1", "0123456789"), put(2, "k2", "0123456789")))}); d != "" {
		t.Fatalf("rule 9: two records fit exactly into SegmentSize = 16+2R: %s", d)
	}
	m.trace = nil
	l.Put("k3", []byte("0123456789"))
	want := []string{"close 00000001.seg", "create 00000002.seg", "write 00000002.seg 16", "sync 00000002.seg", fmt.Sprintf("write 00000002.seg %d", r), "sync 00000002.seg"}
	if fmt.Sprint(m.trace) != fmt.Sprint(want) {
		t.Fatalf("rules 9, 10: a rotation must make the calls\n  %q\nit made\n  %q", want, m.trace)
	}
	if m.openCount() != 1 {
		t.Fatalf("rule 15: after a rotation exactly one File is open, not %d", m.openCount())
	}
	// A record bigger than SegmentSize goes into the empty active segment.
	big := strings.Repeat("B", int(3*r))
	l.Put("big", []byte(big))
	l.Put("k4", []byte("x"))
	l.Put("k5", []byte("y"))
	exp := map[string]string{
		seg(1): string(cat(hdr(false, 0), put(1, "k1", "0123456789"), put(2, "k2", "0123456789"))),
		seg(2): string(cat(hdrL(crcOf(put(2, "k2", "0123456789"))), put(3, "k3", "0123456789"))),
		seg(3): string(cat(hdrL(crcOf(put(3, "k3", "0123456789"))), put(4, "big", big))),
		seg(4): string(cat(hdrL(crcOf(put(4, "big", big))), put(5, "k4", "x"), put(6, "k5", "y"))),
	}
	if d := dirDiff(m.dir(), exp); d != "" {
		t.Fatalf("rule 9: %s", d)
	}
	l.Close()
	// After reopening, the size of the existing active segment counts.
	l = mustOpen(t, m, 16+2*r)
	l.Put("k6", []byte("z"))
	exp[seg(5)] = string(cat(hdrL(crcOf(put(6, "k5", "y"))), put(7, "k6", "z")))
	if d := dirDiff(m.dir(), exp); d != "" {
		t.Fatalf("rule 9 after reopen: %s", d)
	}
	l.Close()
	// A different SegmentSize on reopen applies from then on.
	l = mustOpen(t, m, 1000)
	l.Put("k7", []byte("w"))
	exp[seg(5)] = string(cat(hdrL(crcOf(put(6, "k5", "y"))), put(7, "k6", "z"), put(8, "k7", "w")))
	if d := dirDiff(m.dir(), exp); d != "" {
		t.Fatalf("rule 9 with a larger SegmentSize: %s", d)
	}
	l.Close()
}

func TestDefaultSegmentSize(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	v := bytes.Repeat([]byte{1}, wal.MaxValueSize)
	for i := 0; i < 4; i++ {
		l.Put(fmt.Sprint(i), v)
	}
	d := m.dir()
	r := 8 + 11 + 1 + wal.MaxValueSize
	if len(d) != 2 || len(d[seg(1)]) != 16+3*r || len(d[seg(2)]) != 16+r {
		t.Fatalf("rule 9: with SegmentSize 0 (DefaultSegmentSize, 4 MiB) three records of 1 MiB fit into the first segment and the fourth starts the second; found %d segments, the first of %d bytes", len(d), len(d[seg(1)]))
	}
	l.Close()
}

// ---------------------------------------------------------------------------
// Rules 17 to 26: what Open makes of a directory.

type dirCase struct {
	raw     bool // use the files as they are, without chained()
	name    string
	files   map[string][]byte
	corrupt string // segment name when Open must fail
	offset  int64
	state   st
	after   map[string][]byte // directory after a successful Open
	active  string            // segment that the next Put appends to
}

func runDirCase(t *testing.T, c dirCase) {
	t.Helper()
	if !c.raw {
		c.files = chained(c.files)
		if c.after != nil {
			c.after = chained(c.after)
		}
	}
	m := memWith(c.files)
	before := m.dir()
	l, err := wal.Open("db", wal.Options{FS: m})
	if c.corrupt != "" {
		var ce *wal.CorruptError
		if err == nil {
			t.Fatalf("%s: Open succeeded with %v; want *CorruptError{%s, %d}", c.name, observe(l), c.corrupt, c.offset)
		}
		if !errors.As(err, &ce) || !errors.Is(err, wal.ErrCorrupt) {
			t.Fatalf("%s: Open returned %v; want *CorruptError{%s, %d} (rule 18)", c.name, err, c.corrupt, c.offset)
		}
		if ce.Segment != c.corrupt || ce.Offset != c.offset {
			t.Fatalf("%s: Open returned CorruptError{%q, %d}; want {%q, %d} (rules 18, 21)", c.name, ce.Segment, ce.Offset, c.corrupt, c.offset)
		}
		if d := dirDiff(m.dir(), before); d != "" {
			t.Fatalf("%s: rule 25: the directory must be unchanged: %s", c.name, d)
		}
		if m.openCount() != 0 {
			t.Fatalf("%s: rule 15: a File was left open after Open failed", c.name)
		}
		return
	}
	if err != nil {
		t.Fatalf("%s: Open returned %v; want success with %v", c.name, err, c.state)
	}
	if got := observe(l); !same(got, c.state) {
		t.Fatalf("%s: state is %v, want %v", c.name, got, c.state)
	}
	if len(m.badReads) > 0 {
		t.Fatalf("%s: rule 2: foreign file %s was read", c.name, m.badReads[0])
	}
	want := strs(c.after)
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("%s: directory after Open: %s", c.name, d)
	}
	if m.openCount() != 1 {
		t.Fatalf("%s: rule 15: an open log holds exactly one open File, not %d", c.name, m.openCount())
	}
	next := c.state.last + 1
	seq, err := l.Put("next", []byte("N"))
	if err != nil || seq != next {
		t.Fatalf("%s: Put after Open = %d, %v; want %d", c.name, seq, err, next)
	}
	want[c.active] += string(put(next, "next", "N"))
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("%s: rule 26: the next record goes to %s: %s", c.name, c.active, d)
	}
	l.Close()
	l2, err := wal.Open("db", wal.Options{FS: m})
	if err != nil {
		t.Fatalf("%s: second Open: %v", c.name, err)
	}
	exp := c.state.clone()
	exp.kv["next"] = "N"
	exp.last = next
	if got := observe(l2); !same(got, exp) {
		t.Fatalf("%s: after a Put and a reopen the state is %v, want %v", c.name, got, exp)
	}
	l2.Close()
}

func TestTornTail(t *testing.T) {
	r1, r2, r3 := put(1, "a", "one"), put(2, "b", "two-two"), del(3, "a")
	full := cat(hdr(false, 0), r1, r2, r3)
	bounds := []int{16, 16 + len(r1), 16 + len(r1) + len(r2), len(full)}
	states := []st{mk(0), mk(1, "a", "one"), mk(2, "a", "one", "b", "two-two"), mk(3, "b", "two-two")}
	for cut := 16; cut <= len(full); cut++ {
		k := 0
		for k+1 < len(bounds) && bounds[k+1] <= cut {
			k++
		}
		runDirCase(t, dirCase{
			name:   fmt.Sprintf("rules 21-22: highest segment cut to %d bytes", cut),
			files:  map[string][]byte{seg(1): full[:cut]},
			state:  states[k],
			after:  map[string][]byte{seg(1): full[:bounds[k]]},
			active: seg(1),
		})
		// The same cut in a segment that is not the highest.
		c := dirCase{
			name:  fmt.Sprintf("rule 22: segment 1 of 2 cut to %d bytes", cut),
			files: map[string][]byte{seg(1): full[:cut], seg(2): cat(hdr(false, 0), put(4, "c", "x"))},
		}
		if bounds[k] == cut {
			s := states[k].clone()
			s.kv["c"] = "x"
			s.last = 4
			c.state, c.after, c.active = s, c.files, seg(2)
		} else {
			c.corrupt, c.offset = seg(1), int64(bounds[k])
		}
		runDirCase(t, c)
	}
}

func TestBadRecords(t *testing.T) {
	r1, r2, r3 := put(1, "a", "one"), put(2, "b", "two"), put(3, "c", "three")
	flip := func(b []byte, i int) []byte { b = append([]byte(nil), b...); b[i] ^= 0x20; return b }
	h := hdr(false, 0)
	p2 := int64(16 + len(r1))
	p3 := p2 + int64(len(r2))
	nextSeg := map[string][]byte{seg(2): cat(h, put(9, "z", "z"))}
	with := func(m map[string][]byte, extra map[string][]byte) map[string][]byte {
		out := map[string][]byte{}
		for k, v := range m {
			out[k] = v
		}
		for k, v := range extra {
			out[k] = v
		}
		return out
	}
	lenField := func(n uint32, tail int) []byte {
		b := make([]byte, 8+tail)
		binary.LittleEndian.PutUint32(b, n)
		return b
	}
	cases := []dirCase{
		{name: "rule 21: CRC mismatch in the last record of the highest segment is torn",
			files: map[string][]byte{seg(1): cat(h, r1, r2, flip(r3, 20))},
			state: mk(2, "a", "one", "b", "two"), after: map[string][]byte{seg(1): cat(h, r1, r2)}, active: seg(1)},
		{name: "rule 21: CRC mismatch in a middle record is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, flip(r2, 20), r3)}, corrupt: seg(1), offset: p2},
		{name: "rule 21: a flipped checksum field in a middle record is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, flip(r2, 5), r3)}, corrupt: seg(1), offset: p2},
		{name: "rule 22: CRC mismatch in the last record of a lower segment is corrupt",
			files: with(map[string][]byte{seg(1): cat(h, r1, r2, flip(r3, 20))}, nextSeg), corrupt: seg(1), offset: p3},
		{name: "rule 21: N < 11 at the end of the highest segment is corrupt, not torn",
			files: map[string][]byte{seg(1): cat(h, r1, r2, lenField(5, 0))}, corrupt: seg(1), offset: p3},
		{name: "rule 21: N = 10 with bytes after it is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, r2, lenField(10, 30))}, corrupt: seg(1), offset: p3},
		{name: "rule 21: N > MaxPayload at the end is corrupt, not torn",
			files: map[string][]byte{seg(1): cat(h, r1, r2, lenField(wal.MaxPayload+1, 3))}, corrupt: seg(1), offset: p3},
		{name: "rule 21: N = MaxPayload with too few bytes is torn",
			files: map[string][]byte{seg(1): cat(h, r1, r2, lenField(wal.MaxPayload, 40))},
			state: mk(2, "a", "one", "b", "two"), after: map[string][]byte{seg(1): cat(h, r1, r2)}, active: seg(1)},
		{name: "rule 21: fewer than 8 bytes at the end are torn",
			files: map[string][]byte{seg(1): cat(h, r1, r2, []byte{0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff})},
			state: mk(2, "a", "one", "b", "two"), after: map[string][]byte{seg(1): cat(h, r1, r2)}, active: seg(1)},
		{name: "rule 21: kind 3 with a good checksum is corrupt, also at the end",
			files: map[string][]byte{seg(1): cat(h, r1, r2, frame(payload(3, 3, 1, "c", "v")))}, corrupt: seg(1), offset: p3},
		{name: "rule 21: kind 0 is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, frame(payload(2, 0, 1, "b", "")), r3)}, corrupt: seg(1), offset: p2},
		{name: "rule 21: K > N-11 is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, frame(payload(2, 1, 9, "b", "two")), r3)}, corrupt: seg(1), offset: p2},
		{name: "rule 21: a delete with a value is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, r2, frame(payload(3, 2, 1, "a", "v")))}, corrupt: seg(1), offset: p3},
		{name: "rule 18: the first of several problems is reported",
			files: map[string][]byte{seg(1): cat(h, r1, frame(payload(2, 1, 9, "b", "two")), flip(r3, 20)), seg(2): []byte("garbage that is not a header")}, corrupt: seg(1), offset: p2},
		{name: "rule 18: a problem in a lower segment comes first",
			files: map[string][]byte{seg(1): cat(h, r1, flip(r2, 20), r3), seg(2): rawHdr("XSEG", 1, 0, 0)}, corrupt: seg(1), offset: p2},
		{name: "rule 25: a corrupt segment leaves leftovers and torn creations alone",
			files: map[string][]byte{seg(1): cat(h, r1, flip(r2, 20), r3), seg(2): cat(h, r3), seg(3): []byte("WSE"), "00000004.seg.tmp": []byte("x"), "x.tmp": nil}, corrupt: seg(1), offset: p2},
		{name: "rule 22: a torn tail in the highest of two segments",
			files: map[string][]byte{seg(4): cat(h, r1), seg(5): cat(h, r2, r3[:5]), "x.tmp": nil},
			state: mk(2, "a", "one", "b", "two"), after: map[string][]byte{seg(4): cat(h, r1), seg(5): cat(h, r2)}, active: seg(5)},
	}
	cases = append(cases, dirCase{name: "rule 25: corrupt segment below a torn tail",
		files:   map[string][]byte{seg(4): cat(h, r1, flip(r2, 20), r3), seg(5): cat(h, r3[:5]), "x.tmp": nil},
		corrupt: seg(4), offset: p2})
	for _, c := range cases {
		runDirCase(t, c)
	}
}

func TestHeaders(t *testing.T) {
	r1 := put(1, "a", "1")
	good := cat(hdr(false, 0), r1)
	bad := map[string][]byte{
		"magic":             rawHdr("WSEX", 1, 0, 0),
		"version 2":         rawHdr("WSEG", 2, 0, 0),
		"version 0":         rawHdr("WSEG", 0, 0, 0),
		"flag bit 1":        rawHdr("WSEG", 1, 2, 0),
		"flags 3":           rawHdr("WSEG", 1, 3, 9),
		"bytes 12-15 not 0": rawHdr("WSEG", 1, 0, 5<<32),
	}
	names := make([]string, 0, len(bad))
	for n := range bad {
		names = append(names, n)
	}
	sort.Strings(names)
	for _, n := range names {
		hb := bad[n]
		runDirCase(t, dirCase{name: "rule 19: highest segment with header " + n,
			files: map[string][]byte{seg(1): good, seg(2): cat(hb, put(2, "b", "2"))}, corrupt: seg(2), offset: 0})
		runDirCase(t, dirCase{name: "rule 19: lower segment with header " + n,
			files: map[string][]byte{seg(1): cat(hb, r1), seg(2): cat(hdr(false, 0), put(2, "b", "2"))}, corrupt: seg(1), offset: 0})
		runDirCase(t, dirCase{name: "rule 19: header " + n + " above a snapshot",
			files: map[string][]byte{seg(1): cat(hdr(true, 1), r1), seg(2): hb}, corrupt: seg(2), offset: 0})
	}
}

func TestTornCreation(t *testing.T) {
	r1 := put(1, "a", "1")
	good := cat(hdr(false, 0), r1)
	for _, n := range []int{0, 1, 7, 15} {
		short := hdr(false, 0)[:n]
		if n == 7 {
			short = []byte("garbage")
		}
		runDirCase(t, dirCase{name: fmt.Sprintf("rule 20: highest segment of %d bytes", n),
			files:  map[string][]byte{seg(1): good, seg(2): short, "00000003.seg.tmp": []byte("left")},
			state:  mk(1, "a", "1"),
			after:  map[string][]byte{seg(1): good, seg(2): hdr(false, 0)},
			active: seg(2)})
		runDirCase(t, dirCase{name: fmt.Sprintf("rule 20: lower segment of %d bytes", n),
			files: map[string][]byte{seg(1): short, seg(2): good}, corrupt: seg(1), offset: 0})
		runDirCase(t, dirCase{name: fmt.Sprintf("rule 20: the only segment has %d bytes", n),
			files:  map[string][]byte{seg(7): short},
			state:  mk(0),
			after:  map[string][]byte{seg(7): hdr(false, 0)},
			active: seg(7)})
		runDirCase(t, dirCase{name: fmt.Sprintf("rule 20: a %d-byte segment above a snapshot", n),
			files:  map[string][]byte{seg(3): good, seg(4): cat(hdr(true, 6), put(2, "s", "S")), seg(5): short},
			state:  mk(6, "s", "S"),
			after:  map[string][]byte{seg(4): cat(hdr(true, 6), put(2, "s", "S")), seg(5): hdr(false, 0)},
			active: seg(5)})
	}
	m := newMem()
	m.dirs["db"] = true
	l := mustOpen(t, m, 0)
	l.Close()
	if d := dirDiff(m.dir(), map[string]string{seg(1): string(hdr(false, 0))}); d != "" {
		t.Fatalf("rule 26: %s", d)
	}
}

func TestSnapshots(t *testing.T) {
	h := hdr(false, 0)
	snap := cat(hdr(true, 9), put(4, "a", "A"), put(7, "b", "B"))
	cases := []dirCase{
		{name: "rules 17, 24, 26: obsolete segments are ignored and removed, stale records skipped",
			files: map[string][]byte{seg(1): []byte("garbage"), seg(2): cat(rawHdr("JUNK", 9, 9, 9), []byte("more garbage")), seg(3): snap,
				seg(4): cat(h, put(8, "c", "stale"), put(9, "a", "stale"), put(10, "d", "D"), del(6, "b"), del(11, "a"))},
			state: mk(11, "b", "B", "d", "D"),
			after: map[string][]byte{seg(3): snap, seg(4): cat(h, put(8, "c", "stale"), put(9, "a", "stale"), put(10, "d", "D"), del(6, "b"), del(11, "a"))}, active: seg(4)},
		{name: "rule 26: a snapshot as the highest segment gets a new active segment after it",
			files: map[string][]byte{seg(2): cat(h, put(1, "x", "X")), seg(5): cat(hdr(true, 12))},
			state: mk(12), after: map[string][]byte{seg(5): hdr(true, 12), seg(6): h}, active: seg(6)},
		{name: "rule 24: a snapshot sets LastSeq to its header's number",
			files: map[string][]byte{seg(1): snap},
			state: mk(9, "a", "A", "b", "B"), after: map[string][]byte{seg(1): snap, seg(2): h}, active: seg(2)},
		{name: "rule 17: only the highest snapshot counts",
			files: map[string][]byte{seg(1): cat(hdr(true, 3), put(3, "old", "O")), seg(2): cat(h, put(4, "mid", "M")), seg(3): snap, seg(4): cat(h, put(10, "e", "E"))},
			state: mk(10, "a", "A", "b", "B", "e", "E"), after: map[string][]byte{seg(3): snap, seg(4): cat(h, put(10, "e", "E"))}, active: seg(4)},
		{name: "rule 17: a snapshot flag in an invalid header does not make a snapshot",
			files: map[string][]byte{seg(1): snap, seg(2): rawHdr("WSEG", 2, 1, 20)}, corrupt: seg(2), offset: 0},
		{name: "rule 17: a lower segment that is not obsolete is checked",
			files: map[string][]byte{seg(1): cat(h, put(1, "a", "1"), []byte{1, 2, 3}), seg(2): cat(h, put(2, "b", "2"))}, corrupt: seg(1), offset: 16 + int64(len(put(1, "a", "1")))},
		{name: "rule 23: a delete in a snapshot is corrupt",
			files: map[string][]byte{seg(1): cat(hdr(true, 9), put(4, "a", "A"), del(5, "a"))}, corrupt: seg(1), offset: 16 + int64(len(put(4, "a", "A")))},
		{name: "rule 23: equal sequence numbers in a snapshot are corrupt",
			files: map[string][]byte{seg(1): cat(hdr(true, 9), put(4, "a", "A"), put(4, "b", "B"))}, corrupt: seg(1), offset: 16 + int64(len(put(4, "a", "A")))},
		{name: "rule 23: decreasing sequence numbers in a snapshot are corrupt",
			files: map[string][]byte{seg(1): cat(hdr(true, 9), put(4, "a", "A"), put(2, "b", "B"))}, corrupt: seg(1), offset: 16 + int64(len(put(4, "a", "A")))},
		{name: "rule 23: a sequence number above the header's is corrupt",
			files: map[string][]byte{seg(1): cat(hdr(true, 9), put(4, "a", "A"), put(10, "b", "B"))}, corrupt: seg(1), offset: 16 + int64(len(put(4, "a", "A")))},
		{name: "rule 22: a torn record in a snapshot is corrupt",
			files: map[string][]byte{seg(1): snap[:len(snap)-3]}, corrupt: seg(1), offset: 16 + int64(len(put(4, "a", "A")))},
		{name: "rule 22: a CRC mismatch at the end of a snapshot is corrupt",
			files: map[string][]byte{seg(1): func() []byte { b := append([]byte(nil), snap...); b[len(b)-1] ^= 1; return b }()}, corrupt: seg(1), offset: 16 + int64(len(put(4, "a", "A")))},
		{name: "rule 22: fewer than 8 bytes after a snapshot's records are corrupt",
			files: map[string][]byte{seg(1): cat(snap, []byte{1, 2})}, corrupt: seg(1), offset: int64(len(snap))},
		{name: "rule 24: duplicate sequence numbers: the first wins",
			files: map[string][]byte{seg(1): cat(h, put(1, "a", "x"), put(1, "a", "y"), put(3, "b", "1"), put(2, "c", "1"), del(3, "b")),
				seg(2): cat(h, put(3, "b", "again"), put(4, "a", "z"), put(4, "q", "no"))},
			state: mk(4, "a", "z", "b", "1"),
			after: map[string][]byte{seg(1): cat(h, put(1, "a", "x"), put(1, "a", "y"), put(3, "b", "1"), put(2, "c", "1"), del(3, "b")),
				seg(2): cat(h, put(3, "b", "again"), put(4, "a", "z"), put(4, "q", "no"))}, active: seg(2)},
		{name: "rule 24: gaps in sequence numbers are fine",
			files: map[string][]byte{seg(1): cat(h, put(5, "a", "x"), put(50, "b", "y"))},
			state: mk(50, "a", "x", "b", "y"), after: map[string][]byte{seg(1): cat(h, put(5, "a", "x"), put(50, "b", "y"))}, active: seg(1)},
	}
	for _, c := range cases {
		runDirCase(t, c)
	}
}

func TestForeignAndLeftovers(t *testing.T) {
	h := hdr(false, 0)
	foreign := map[string][]byte{
		"notes.txt": []byte("hello"), "1.seg": []byte("x"), "00000000.seg": []byte("zero"),
		"000000012.seg": []byte("nine digits"), "00000002.SEG": []byte("upper"), "00000003.seg.bak": []byte("bak"),
		"0000000a.seg": []byte("hex"), "+0000001.seg": []byte("sign"),
	}
	files := map[string][]byte{seg(5): cat(h, put(1, "a", "1")), "00000009.seg.tmp": []byte("left"), "junk.tmp": []byte("j")}
	for n, b := range foreign {
		files[n] = b
	}
	m := memWith(files)
	for n := range foreign {
		m.forbid["db/"+n] = true
	}
	l, err := wal.Open("db", wal.Options{FS: m})
	if err != nil {
		t.Fatalf("rule 2: Open with foreign files returned %v", err)
	}
	if len(m.badReads) > 0 {
		t.Fatalf("rule 2: foreign file %s was read", m.badReads[0])
	}
	want := strs(foreign)
	want[seg(5)] = string(cat(h, put(1, "a", "1")))
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rules 2, 26: leftovers are removed, foreign files kept: %s", d)
	}
	wantState(t, l, mk(1, "a", "1"), "rule 24")
	l.Put("b", []byte("2"))
	if err := l.Compact(); err != nil {
		t.Fatal(err)
	}
	want = strs(foreign)
	want[seg(6)] = string(cat(hdr(true, 2), put(1, "a", "1"), put(2, "b", "2")))
	want[seg(7)] = string(hdrL(crcOf(put(2, "b", "2"))))
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rule 29: after Compact: %s", d)
	}
	if len(m.badReads) > 0 {
		t.Fatalf("rule 2: foreign file %s was read", m.badReads[0])
	}
	l.Close()
	l = mustOpen(t, m, 0)
	wantState(t, l, mk(2, "a", "1", "b", "2"), "rule 24 after Compact and reopen")
	l.Close()
	if len(m.badReads) > 0 {
		t.Fatalf("rule 2: foreign file %s was read", m.badReads[0])
	}
}

// ---------------------------------------------------------------------------
// Rules 28 and 29.

func TestCompactBytes(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 60)
	l.Put("a", []byte("1"))                                                      // 1
	l.Put("b", []byte("2"))                                                      // 2
	l.Put("a", []byte("3"))                                                      // 3
	l.Delete("b")                                                                // 4
	l.Put("c", []byte("5"))                                                      // 5
	l.Apply(wal.Record{Seq: 9, Kind: wal.KindPut, Key: "d", Value: []byte("9")}) // 9
	l.Put("b", []byte("x"))                                                      // 10
	l.Delete("zz")                                                               // 11
	segs := 0
	for n := range m.dir() {
		if strings.HasSuffix(n, ".seg") {
			segs++
		}
	}
	if segs < 3 {
		t.Fatalf("rule 9: with SegmentSize 60 eight records need several segments, found %d", segs)
	}
	a := segs // segments are 1..segs
	m.trace = nil
	if err := l.Compact(); err != nil {
		t.Fatalf("Compact: %v", err)
	}
	snap := cat(hdr(true, 11), put(3, "a", "3"), put(5, "c", "5"), put(9, "d", "9"), put(10, "b", "x"))
	want := map[string]string{seg(a + 1): string(snap), seg(a + 2): string(hdrL(crcOf(put(10, "b", "x"))))}
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rules 28, 29: %s", d)
	}
	tmp := seg(a+1) + ".tmp"
	head := []string{"create " + tmp, fmt.Sprintf("write %s %d", tmp, len(snap)), "sync " + tmp, "close " + tmp,
		"rename " + tmp + " " + seg(a+1), "close " + seg(a), "create " + seg(a+2), "write " + seg(a+2) + " 16", "sync " + seg(a+2)}
	var removes []string
	for i := 1; i <= a; i++ {
		removes = append(removes, "remove "+seg(i))
	}
	tr := append([]string(nil), m.trace...)
	ok := len(tr) == len(head)+len(removes) && fmt.Sprint(tr[:len(head)]) == fmt.Sprint(head)
	if ok {
		rest := append([]string(nil), tr[len(head):]...)
		sort.Strings(rest)
		ok = fmt.Sprint(rest) == fmt.Sprint(removes)
	}
	if !ok {
		t.Fatalf("rule 28: Compact must make these calls in this order (the removes in any order):\n  %q\n  %q\nit made\n  %q", head, removes, tr)
	}
	wantState(t, l, mk(11, "a", "3", "b", "x", "c", "5", "d", "9"), "rule 29")
	if m.openCount() != 1 {
		t.Fatal("rule 15: after Compact exactly one File is open")
	}
	l.Close()
	l = mustOpen(t, m, 60)
	wantState(t, l, mk(11, "a", "3", "b", "x", "c", "5", "d", "9"), "rule 29 (reopen)")
	if seq, _ := l.Put("e", []byte("12")); seq != 12 {
		t.Fatalf("rule 24: after a snapshot with sequence number 11, Put returns %d, want 12", seq)
	}
	l.Put("c", []byte("13"))
	if err := l.Compact(); err != nil {
		t.Fatal(err)
	}
	snap2 := cat(hdr(true, 13), put(3, "a", "3"), put(9, "d", "9"), put(10, "b", "x"), put(12, "e", "12"), put(13, "c", "13"))
	want = map[string]string{seg(a + 3): string(snap2), seg(a + 4): string(hdrL(crcOf(put(13, "c", "13"))))}
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rules 28, 29 (second Compact, sequence numbers survive): %s", d)
	}
	l.Close()
	// Compacting an empty log and compacting twice in a row.
	m = newMem()
	l = mustOpen(t, m, 0)
	if err := l.Compact(); err != nil {
		t.Fatal(err)
	}
	if err := l.Compact(); err != nil {
		t.Fatal(err)
	}
	want = map[string]string{seg(4): string(hdr(true, 0)), seg(5): string(hdr(false, 0))}
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rules 28, 29 (empty log compacted twice): %s", d)
	}
	l.Close()
	l = mustOpen(t, m, 0)
	wantState(t, l, mk(0), "rule 24")
	l.Close()
}

// ---------------------------------------------------------------------------
// Rules 13, 15, 27, 30, 31, 32, 33: failures and crashes at every step.

type wl struct {
	m    *memFS
	l    *wal.Log
	size int64
}

type mismatch struct{ msg string }

func (e mismatch) Error() string { return e.msg }

type call struct {
	name  string
	kind  string // open, close, mut, compact
	run   func(w *wl, before st) error
	apply func(s *st)
}

func cOpen() call {
	return call{"Open", "open", func(w *wl, _ st) error {
		l, err := wal.Open("db", wal.Options{FS: w.m, SegmentSize: w.size})
		w.l = l
		return err
	}, nil}
}

func cClose() call {
	return call{"Close", "close", func(w *wl, _ st) error { return w.l.Close() }, nil}
}

func cPut(k, v string) call {
	return call{fmt.Sprintf("Put(%q)", k), "mut", func(w *wl, b st) error {
		seq, err := w.l.Put(k, []byte(v))
		if err == nil && seq != b.last+1 {
			return mismatch{fmt.Sprintf("rule 6: Put returned %d, want %d", seq, b.last+1)}
		}
		return err
	}, func(s *st) { s.kv[k] = v; s.last++ }}
}

func cDel(k string) call {
	return call{fmt.Sprintf("Delete(%q)", k), "mut", func(w *wl, b st) error {
		seq, err := w.l.Delete(k)
		if err == nil && seq != b.last+1 {
			return mismatch{fmt.Sprintf("rule 6: Delete returned %d, want %d", seq, b.last+1)}
		}
		return err
	}, func(s *st) { delete(s.kv, k); s.last++ }}
}

func cApply(r wal.Record) call {
	return call{fmt.Sprintf("Apply(seq %d)", r.Seq), "mut", func(w *wl, b st) error {
		ok, err := w.l.Apply(r)
		if err == nil && ok != (r.Seq > b.last) {
			return mismatch{fmt.Sprintf("rule 7: Apply(seq %d) returned %v with LastSeq %d", r.Seq, ok, b.last)}
		}
		return err
	}, func(s *st) {
		if r.Seq <= s.last {
			return
		}
		if r.Kind == wal.KindPut {
			s.kv[r.Key] = string(r.Value)
		} else {
			delete(s.kv, r.Key)
		}
		s.last = r.Seq
	}}
}

func cBatch(ops ...wal.Op) call {
	return call{fmt.Sprintf("Batch(%d ops)", len(ops)), "mut", func(w *wl, b st) error {
		last, err := w.l.Batch(ops)
		if err == nil && last != b.last+uint64(len(ops)) {
			return mismatch{fmt.Sprintf("rule 34: Batch returned %d, want %d", last, b.last+uint64(len(ops)))}
		}
		return err
	}, func(s *st) {
		for _, op := range ops {
			if op.Kind == wal.KindPut {
				s.kv[op.Key] = string(op.Value)
			} else {
				delete(s.kv, op.Key)
			}
			s.last++
		}
	}}
}

func opPut(k, v string) wal.Op { return wal.Op{Kind: wal.KindPut, Key: k, Value: []byte(v)} }
func opDel(k string) wal.Op    { return wal.Op{Kind: wal.KindDelete, Key: k} }

func cCompact() call {
	return call{"Compact", "compact", func(w *wl, _ st) error { return w.l.Compact() }, nil}
}

func workload() []call {
	return []call{
		cOpen(),
		cPut("a", "alpha-1"),
		cPut("b", "beta"),
		cPut("c", strings.Repeat("gamma-", 20)),
		cDel("b"),
		cPut("a", "alpha-2"),
		cDel("nope"),
		cApply(wal.Record{Seq: 20, Kind: wal.KindPut, Key: "d", Value: []byte("delta")}),
		cApply(wal.Record{Seq: 15, Kind: wal.KindPut, Key: "x", Value: []byte("stale")}),
		cPut("", ""),
		cBatch(opPut("b2", "bee"), opDel("c"), opPut("b2", "bee-2")),
		cBatch(opPut("big1", strings.Repeat("1", 50)), opPut("big2", strings.Repeat("2", 50)), opDel("d")),
		cCompact(),
		cPut("e", "echo"),
		cDel("a"),
		cPut("c", "gamma-2"),
		cClose(),
		cOpen(),
		cPut("f", "foxtrot"),
		cBatch(opPut("solo", "S")),
		cBatch(opDel("solo"), opPut("e", "echo-2")),
		cApply(wal.Record{Seq: 30, Kind: wal.KindDelete, Key: "c"}),
		cCompact(),
		cPut("g", "golf"),
		cCompact(),
		cPut("h", "hotel"),
		cClose(),
	}
}

func modelStates(calls []call) []st {
	s := mk(0)
	out := []st{s.clone()}
	for _, c := range calls {
		if c.apply != nil {
			c.apply(&s)
		}
		out = append(out, s.clone())
	}
	return out
}

const wlSize = 90

// recover opens a directory left behind by a failure or a crash and checks
// that it shows one of the allowed states and keeps working.
func recoverCheck(t *testing.T, m *memFS, allowed []st, what string) st {
	t.Helper()
	m.failAt = 0
	l, err := wal.Open("db", wal.Options{FS: m, SegmentSize: wlSize})
	if err != nil {
		t.Fatalf("%s: rule 32/33: Open afterwards returned %v", what, err)
	}
	got := observe(l)
	okState := false
	for _, a := range allowed {
		if same(got, a) {
			okState = true
		}
	}
	if !okState {
		t.Fatalf("%s: rule 32/33: Open afterwards shows %v; allowed: %v", what, got, allowed)
	}
	if m.openCount() != 1 {
		t.Fatalf("%s: rule 15: after Open %d Files are open, want 1", what, m.openCount())
	}
	if err := l.Close(); err != nil || m.openCount() != 0 {
		t.Fatalf("%s: rule 14/15: Close returned %v and left %d Files open", what, err, m.openCount())
	}
	return got
}

func continueCheck(t *testing.T, m *memFS, got st, what string) {
	t.Helper()
	m.record = false
	l, err := wal.Open("db", wal.Options{FS: m, SegmentSize: wlSize})
	if err != nil {
		t.Fatalf("%s: Open: %v", what, err)
	}
	seq, err := l.Put("post", []byte(strings.Repeat("p", 40)))
	if err != nil || seq != got.last+1 {
		t.Fatalf("%s: rule 33: the recovered log must work normally; Put = %d, %v; want %d", what, seq, err, got.last+1)
	}
	if err := l.Close(); err != nil {
		t.Fatalf("%s: Close: %v", what, err)
	}
	l, err = wal.Open("db", wal.Options{FS: m, SegmentSize: wlSize})
	if err != nil {
		t.Fatalf("%s: rule 33: Open after recovery and a Put: %v", what, err)
	}
	exp := got.clone()
	exp.kv["post"] = strings.Repeat("p", 40)
	exp.last = seq
	if s := observe(l); !same(s, exp) {
		t.Fatalf("%s: rule 33: after recovery, a Put and a reopen the state is %v, want %v", what, s, exp)
	}
	l.Close()
	if m.openCount() != 0 {
		t.Fatalf("%s: rule 15: Files left open after Close", what)
	}
}

func TestFailures(t *testing.T) {
	calls := workload()
	states := modelStates(calls)
	for k := 1; ; k++ {
		m := newMem()
		m.failAt = k
		w := &wl{m: m, size: wlSize}
		at := -1
		var ferr error
		for i, c := range calls {
			if err := c.run(w, states[i]); err != nil {
				at, ferr = i, err
				break
			}
		}
		if at < 0 {
			if m.failed {
				t.Fatalf("rules 13, 27, 30: the failure of file system call %d was not reported by any call", k)
			}
			if k < 50 {
				t.Fatalf("the workload made only %d file system calls", k-1)
			}
			return
		}
		c := calls[at]
		what := fmt.Sprintf("file system call %d fails, inside call %d %s", k, at, c.name)
		var mm mismatch
		if errors.As(ferr, &mm) {
			t.Fatalf("%s: %s", what, mm.msg)
		}
		if !m.failed {
			t.Fatalf("call %d %s failed without any injected failure: %v", at, c.name, ferr)
		}
		if !errors.Is(ferr, errBoom) {
			t.Fatalf("%s: rules 13, 14, 27, 30: the call returned %v, which does not wrap the file system's error", what, ferr)
		}
		switch c.kind {
		case "open":
			if m.openCount() != 0 {
				t.Fatalf("%s: rule 15: Open failed and left %d Files open", what, m.openCount())
			}
		case "close":
			if m.openCount() != 0 {
				t.Fatalf("%s: rule 14/15: Close failed and left a File open", what)
			}
			if _, err := w.l.Put("z", nil); !errors.Is(err, wal.ErrClosed) {
				t.Fatalf("%s: rule 14: the log is closed even when Close fails; Put returned %v", what, err)
			}
		default:
			if got := observe(w.l); !same(got, states[at]) {
				t.Fatalf("%s: rule 13/30: state after the failure is %v, want %v", what, got, states[at])
			}
			if n := m.openCount(); n > 1 {
				t.Fatalf("%s: rule 15: a poisoned log holds at most one open File, it holds %d", what, n)
			}
			ops := m.ops
			if _, err := w.l.Put("z", []byte("z")); !errors.Is(err, wal.ErrPoisoned) {
				t.Fatalf("%s: rule 13: Put after the failure returned %v, want ErrPoisoned", what, err)
			}
			if err := w.l.Compact(); !errors.Is(err, wal.ErrPoisoned) {
				t.Fatalf("%s: rule 13: Compact after the failure returned %v, want ErrPoisoned", what, err)
			}
			if m.ops != ops {
				t.Fatalf("%s: rule 13: a poisoned log must not call the file system", what)
			}
			if err := w.l.Close(); err != nil {
				t.Fatalf("%s: rule 13: Close of a poisoned log returned %v", what, err)
			}
			if m.openCount() != 0 {
				t.Fatalf("%s: rule 15: Files left open after Close of a poisoned log", what)
			}
		}
		got := recoverCheck(t, m, []st{states[at], states[at+1]}, what)
		continueCheck(t, m, got, what)
	}
}

func TestCrashes(t *testing.T) {
	calls := workload()
	states := modelStates(calls)
	m := newMem()
	m.record = true
	w := &wl{m: m, size: wlSize}
	for i, c := range calls {
		m.tag = i
		if err := c.run(w, states[i]); err != nil {
			t.Fatalf("workload call %d %s: %v", i, c.name, err)
		}
	}
	if len(m.images) < 40 {
		t.Fatalf("the workload changed the file system only %d times", len(m.images))
	}
	for j, img := range m.images {
		for v := 0; v < 3; v++ {
			what := fmt.Sprintf("crash after file system change %d (inside call %d %s), unsynced data kept: %s",
				j+1, img.tag, calls[img.tag].name, []string{"none", "all", "half"}[v])
			cm := img.fs(v)
			cm.record = true
			got := recoverCheck(t, cm, []st{states[img.tag], states[img.tag+1]}, what)
			// A second crash, inside the recovering Open.
			for j2, img2 := range cm.images {
				for _, v2 := range []int{0, 2} {
					what2 := fmt.Sprintf("%s; then a crash after change %d of the recovering Open", what, j2+1)
					cm2 := img2.fs(v2)
					recoverCheck(t, cm2, []st{got}, what2)
				}
			}
			continueCheck(t, cm, got, what)
		}
	}
}

// messy builds a directory that Open has work to do in.
func messy(kind int) (map[string][]byte, st) {
	h := hdr(false, 0)
	files := map[string][]byte{
		seg(1):             cat(h, put(1, "old", "O")),
		seg(2):             cat(hdr(true, 4), put(2, "k", "K"), put(4, "m", "M")),
		seg(3):             cat(h, put(5, "n", "N"), del(6, "k")),
		"00000004.seg.tmp": []byte("leftover"),
		"other.tmp":        nil,
		"notes.txt":        []byte("foreign"),
	}
	want := mk(6, "m", "M", "n", "N")
	switch kind {
	case 0: // torn tail
		files[seg(3)] = cat(files[seg(3)], put(7, "t", "torn")[:9])
	case 1: // torn creation
		files[seg(4)] = []byte("WSEG")
	case 2: // the snapshot is the highest segment
		delete(files, seg(3))
		want = mk(4, "k", "K", "m", "M")
	}
	return chained(files), want
}

func TestOpenFailuresAndCrashes(t *testing.T) {
	for kind := 0; kind < 3; kind++ {
		files, want := messy(kind)
		for k := 1; ; k++ {
			m := memWith(files)
			m.failAt = k
			l, err := wal.Open("db", wal.Options{FS: m, SegmentSize: wlSize})
			what := fmt.Sprintf("directory %d, file system call %d of Open fails", kind, k)
			if err == nil {
				if m.failed {
					t.Fatalf("%s: rule 27: Open succeeded although the file system failed", what)
				}
				if s := observe(l); !same(s, want) {
					t.Fatalf("directory %d: Open shows %v, want %v", kind, s, want)
				}
				l.Close()
				break
			}
			if !errors.Is(err, errBoom) {
				t.Fatalf("%s: rule 27: Open returned %v", what, err)
			}
			if m.openCount() != 0 {
				t.Fatalf("%s: rule 15: Open failed and left a File open", what)
			}
			got := recoverCheck(t, m, []st{want}, what)
			continueCheck(t, m, got, what)
		}
		m := memWith(files)
		m.record = true
		l, err := wal.Open("db", wal.Options{FS: m, SegmentSize: wlSize})
		if err != nil {
			t.Fatalf("directory %d: Open: %v", kind, err)
		}
		l.Close()
		if len(m.images) == 0 {
			t.Fatalf("directory %d: Open changed nothing", kind)
		}
		for j, img := range m.images {
			for v := 0; v < 3; v++ {
				what := fmt.Sprintf("directory %d: crash after change %d of Open (unsynced kept: %d)", kind, j+1, v)
				cm := img.fs(v)
				got := recoverCheck(t, cm, []st{want}, what)
				continueCheck(t, cm, got, what)
			}
		}
	}
}

// ---------------------------------------------------------------------------
// The real file system (rule 1: a nil FS means OSFS).

func TestOSFS(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "sub", "db")
	l, err := wal.Open(dir, wal.Options{SegmentSize: 80})
	if err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 6; i++ {
		if _, err := l.Put(fmt.Sprintf("key%d", i), []byte(strings.Repeat("v", 10+i))); err != nil {
			t.Fatal(err)
		}
	}
	l.Delete("key0")
	if err := l.Close(); err != nil {
		t.Fatal(err)
	}
	read := func() map[string]string {
		out := map[string]string{}
		ents, err := os.ReadDir(dir)
		if err != nil {
			t.Fatal(err)
		}
		for _, e := range ents {
			b, _ := os.ReadFile(filepath.Join(dir, e.Name()))
			out[e.Name()] = string(b)
		}
		return out
	}
	files := read()
	var names []string
	for n := range files {
		names = append(names, n)
	}
	sort.Strings(names)
	if len(names) < 3 {
		t.Fatalf("rule 9: expected several segments on disk, found %q", names)
	}
	last := names[len(names)-1]
	lastData := files[last]
	// Cut the last record short, as a crash would.
	if err := os.Truncate(filepath.Join(dir, last), int64(len(lastData)-3)); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(filepath.Join(dir, "notes.txt"), []byte("mine"), 0o644)
	os.WriteFile(filepath.Join(dir, "00000077.seg.tmp"), []byte("left"), 0o644)
	l, err = wal.Open(dir, wal.Options{SegmentSize: 80})
	if err != nil {
		t.Fatalf("rules 21-22 on the real file system: Open returned %v", err)
	}
	if v, ok := l.Get("key0"); !ok || string(v) != "vvvvvvvvvv" || l.LastSeq() != 6 {
		t.Fatalf("rule 22: after cutting the last record (the delete) the state is %v", observe(l))
	}
	after := read()
	if _, ok := after["00000077.seg.tmp"]; ok {
		t.Fatal("rule 26: leftovers are removed")
	}
	if after["notes.txt"] != "mine" {
		t.Fatal("rule 2: foreign files are kept")
	}
	if len(after[last]) != len(lastData)-len(del(7, "key0")) {
		t.Fatalf("rule 22: %s must be cut back to %d bytes, it has %d", last, len(lastData)-len(del(7, "key0")), len(after[last]))
	}
	if _, err := l.Put("key9", []byte("nine")); err != nil {
		t.Fatal(err)
	}
	if err := l.Compact(); err != nil {
		t.Fatal(err)
	}
	if err := l.Close(); err != nil {
		t.Fatal(err)
	}
	after = read()
	if len(after) != 3 || after["notes.txt"] != "mine" {
		t.Fatalf("rule 29 on the real file system: files after Compact: %d", len(after))
	}
	l, err = wal.Open(dir, wal.Options{})
	if err != nil {
		t.Fatal(err)
	}
	want := mk(7, "key0", "vvvvvvvvvv", "key1", "vvvvvvvvvvv", "key2", "vvvvvvvvvvvv", "key3", "vvvvvvvvvvvvv", "key4", "vvvvvvvvvvvvvv", "key5", "vvvvvvvvvvvvvvv", "key9", "nine")
	wantState(t, l, want, "rules 24, 29 on the real file system")
	l.Close()
	// A corrupt middle record on disk: Open fails and changes nothing.
	after = read()
	var segNames []string
	for n := range after {
		if strings.HasSuffix(n, ".seg") {
			segNames = append(segNames, n)
		}
	}
	sort.Strings(segNames)
	snapName := segNames[0]
	b := []byte(after[snapName])
	b[16+8+11+2] ^= 0x40 // inside the first record's payload; more records follow
	os.WriteFile(filepath.Join(dir, snapName), b, 0o644)
	os.WriteFile(filepath.Join(dir, "x.tmp"), nil, 0o644)
	before := read()
	_, err = wal.Open(dir, wal.Options{})
	var ce *wal.CorruptError
	if !errors.As(err, &ce) || ce.Segment != snapName || ce.Offset != 16 {
		t.Fatalf("rules 18, 21 on the real file system: Open returned %v, want CorruptError{%s, 16}", err, snapName)
	}
	if d := dirDiff(read(), before); d != "" {
		t.Fatalf("rule 25 on the real file system: %s", d)
	}
}

// ---------------------------------------------------------------------------
// Rules 34 to 38: batches.

func bput(seq uint64, key, val string) []byte { return frame(payload(seq, 0x81, len(key), key, val)) }
func bdel(seq uint64, key string) []byte      { return frame(payload(seq, 0x82, len(key), key, "")) }

func TestBatchWriting(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	l.Put("a", []byte("1"))
	m.trace = nil
	last, err := l.Batch([]wal.Op{opPut("b", "2"), opDel("a"), opPut("b", "3"), opPut("c", "")})
	if err != nil || last != 5 {
		t.Fatalf("rule 34: Batch = %d, %v; want 5, nil", last, err)
	}
	recs := cat(bput(2, "b", "2"), bdel(3, "a"), bput(4, "b", "3"), put(5, "c", ""))
	want := map[string]string{seg(1): string(cat(hdr(false, 0), put(1, "a", "1"), recs))}
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rule 34: %s", d)
	}
	if fmt.Sprint(m.trace) != fmt.Sprint([]string{fmt.Sprintf("write 00000001.seg %d", len(recs)), "sync 00000001.seg"}) {
		t.Fatalf("rule 36: a batch is one Write followed by one Sync; calls made: %q", m.trace)
	}
	wantState(t, l, mk(5, "b", "3", "c", ""), "rules 34, 36")
	// A batch of one is an ordinary record.
	if last, err := l.Batch([]wal.Op{opDel("c")}); err != nil || last != 6 {
		t.Fatalf("rule 34: Batch of one = %d, %v", last, err)
	}
	want[seg(1)] += string(del(6, "c"))
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rule 34: a batch of one op is written like Delete: %s", d)
	}
	// Copies.
	v := []byte("orig")
	l.Batch([]wal.Op{{Kind: wal.KindPut, Key: "cp", Value: v}, opPut("cp2", "x")})
	v[0] = 'X'
	if g, _ := l.Get("cp"); string(g) != "orig" {
		t.Fatalf("rule 36: Batch must store copies, Get = %q", g)
	}
	l.Close()
	l = mustOpen(t, m, 0)
	wantState(t, l, mk(8, "b", "3", "cp", "orig", "cp2", "x"), "rule 38 (reopen)")
	l.Close()
}

func TestBatchRotation(t *testing.T) {
	m := newMem()
	r := len(put(1, "k", "0123456789"))
	l := mustOpen(t, m, int64(16+3*r))
	l.Put("k", []byte("0123456789"))
	// Two more records of size r would fit, a batch of three does not.
	l.Batch([]wal.Op{opPut("k", "0123456789"), opPut("k", "0123456789"), opPut("k", "0123456789")})
	want := map[string]string{
		seg(1): string(cat(hdr(false, 0), put(1, "k", "0123456789"))),
		seg(2): string(cat(hdrL(crcOf(put(1, "k", "0123456789"))), bput(2, "k", "0123456789"), bput(3, "k", "0123456789"), put(4, "k", "0123456789"))),
	}
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rule 36: a batch goes into one segment, R being its total size: %s", d)
	}
	// A batch bigger than SegmentSize gets a segment of its own.
	ops := []wal.Op{}
	for i := 0; i < 5; i++ {
		ops = append(ops, opPut("k", "0123456789"))
	}
	l.Batch(ops)
	b := hdrL(crcOf(put(4, "k", "0123456789")))
	for i := 0; i < 4; i++ {
		b = cat(b, bput(uint64(5+i), "k", "0123456789"))
	}
	want[seg(3)] = string(cat(b, put(9, "k", "0123456789")))
	if d := dirDiff(m.dir(), want); d != "" {
		t.Fatalf("rules 9, 36: %s", d)
	}
	l.Close()
}

func TestBatchErrors(t *testing.T) {
	m := newMem()
	l := mustOpen(t, m, 0)
	l.Put("keep", []byte("me"))
	big := strings.Repeat("k", wal.MaxKeySize+1)
	bigVal := string(make([]byte, wal.MaxValueSize+1))
	cases := []struct {
		name string
		ops  []wal.Op
		want error
	}{
		{"nil ops", nil, wal.ErrEmptyBatch},
		{"empty ops", []wal.Op{}, wal.ErrEmptyBatch},
		{"second op with a long key", []wal.Op{opPut("a", "1"), opPut(big, "2")}, wal.ErrKeyTooLarge},
		{"second op with a long value", []wal.Op{opPut("a", "1"), opPut("b", bigVal)}, wal.ErrValueTooLarge},
		{"first op with kind 0, second with a long key", []wal.Op{{Kind: 0, Key: "a"}, opPut(big, "")}, wal.ErrInvalidRecord},
		{"first op with a long value, second with a long key", []wal.Op{opPut("a", bigVal), opPut(big, "")}, wal.ErrValueTooLarge},
		{"op with kind 0x81", []wal.Op{opPut("a", "1"), {Kind: 0x81, Key: "b", Value: []byte("v")}}, wal.ErrInvalidRecord},
		{"op with kind 0x82", []wal.Op{{Kind: 0x82, Key: "b"}}, wal.ErrInvalidRecord},
		{"delete op with a value", []wal.Op{opPut("a", "1"), {Kind: wal.KindDelete, Key: "a", Value: []byte("v")}}, wal.ErrInvalidRecord},
		{"delete op with a long key and kind 9 after it", []wal.Op{opDel(big), {Kind: 9, Key: "x"}}, wal.ErrKeyTooLarge},
	}
	for _, c := range cases {
		ops := m.ops
		_, err := l.Batch(c.ops)
		if !errors.Is(err, c.want) {
			t.Fatalf("rule 35: Batch with %s returned %v, want %v", c.name, err, c.want)
		}
		if m.ops != ops {
			t.Fatalf("rule 35: Batch with %s must not call the file system", c.name)
		}
		wantState(t, l, mk(1, "keep", "me"), "rule 35 ("+c.name+" changes nothing)")
	}
	// Poisoned comes before the argument checks, closed before poisoned.
	m.failAt = m.ops + 1
	if _, err := l.Batch([]wal.Op{opPut("a", "1"), opPut("b", "2")}); !errors.Is(err, errBoom) {
		t.Fatalf("rule 36: Batch returned %v when Write failed", err)
	}
	wantState(t, l, mk(1, "keep", "me"), "rule 36 (a failed batch shows none of its ops)")
	if _, err := l.Batch(nil); !errors.Is(err, wal.ErrPoisoned) {
		t.Fatalf("rule 35: empty Batch on a poisoned log returned %v, want ErrPoisoned", err)
	}
	l.Close()
	if _, err := l.Batch(nil); !errors.Is(err, wal.ErrClosed) {
		t.Fatalf("rule 35: empty Batch on a closed log returned %v, want ErrClosed", err)
	}
}

func TestBatchRecovery(t *testing.T) {
	h := hdr(false, 0)
	r1 := put(1, "a", "one")
	batch := cat(bput(2, "b", "two"), bdel(3, "a"), put(4, "c", "three"))
	full := cat(h, r1, batch)
	bstart := 16 + len(r1)
	afterBatch := mk(4, "b", "two", "c", "three")
	for cut := bstart; cut <= len(full); cut++ {
		c := dirCase{name: fmt.Sprintf("rule 37: highest segment with a batch, cut to %d bytes", cut),
			files: map[string][]byte{seg(1): full[:cut]}, active: seg(1)}
		if cut == len(full) {
			c.state, c.after = afterBatch, map[string][]byte{seg(1): full}
		} else {
			c.state, c.after = mk(1, "a", "one"), map[string][]byte{seg(1): full[:bstart]}
		}
		runDirCase(t, c)
		c2 := dirCase{name: fmt.Sprintf("rule 37: lower segment with a batch, cut to %d bytes", cut),
			files: map[string][]byte{seg(1): full[:cut], seg(2): cat(h, put(5, "z", "z"))}}
		if cut == len(full) {
			s := afterBatch.clone()
			s.kv["z"] = "z"
			s.last = 5
			c2.state, c2.after, c2.active = s, c2.files, seg(2)
		} else if cut == bstart {
			c2.state, c2.after, c2.active = mk(5, "a", "one", "z", "z"), c2.files, seg(2)
		} else {
			c2.corrupt, c2.offset = seg(1), int64(bstart)
		}
		runDirCase(t, c2)
	}
	flip := func(b []byte, i int) []byte { b = append([]byte(nil), b...); b[i] ^= 0x10; return b }
	b1, b2, b3 := bput(2, "b", "two"), bdel(3, "a"), put(4, "c", "three")
	p2 := int64(bstart + len(b1))
	cases := []dirCase{
		{name: "rule 37: a CRC mismatch at the end inside a batch tears the whole batch",
			files: map[string][]byte{seg(1): cat(h, r1, b1, b2, flip(b3, 20))},
			state: mk(1, "a", "one"), after: map[string][]byte{seg(1): cat(h, r1)}, active: seg(1)},
		{name: "rule 37: a CRC mismatch in the middle of a batch is corrupt at that record",
			files: map[string][]byte{seg(1): cat(h, r1, b1, flip(b2, 18), b3)}, corrupt: seg(1), offset: p2},
		{name: "rule 37: N < 11 inside an open batch is corrupt at that record",
			files: map[string][]byte{seg(1): cat(h, r1, b1, []byte{3, 0, 0, 0, 0, 0, 0, 0})}, corrupt: seg(1), offset: p2},
		{name: "rule 37: fewer than 8 bytes after an open batch tear it",
			files: map[string][]byte{seg(1): cat(h, r1, b1, b2, []byte{1, 2, 3})},
			state: mk(1, "a", "one"), after: map[string][]byte{seg(1): cat(h, r1)}, active: seg(1)},
		{name: "rule 37: a gap in a batch's sequence numbers is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, b1, bdel(4, "a"), put(5, "c", "three"))}, corrupt: seg(1), offset: p2},
		{name: "rule 37: a repeated sequence number in a batch is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, b1, b2, put(3, "c", "three"))}, corrupt: seg(1), offset: p2 + int64(len(b2))},
		{name: "rule 37: a batch whose last record has a lower number is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, b1, put(1, "c", "x"))}, corrupt: seg(1), offset: p2},
		{name: "rule 21: kind byte 0x83 is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, frame(payload(2, 0x83, 1, "b", "")))}, corrupt: seg(1), offset: int64(bstart)},
		{name: "rule 21: kind byte 0x80 is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, frame(payload(2, 0x80, 1, "b", "")), b3)}, corrupt: seg(1), offset: int64(bstart)},
		{name: "rule 21: a delete with bit 7 and a value is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1, frame(payload(2, 0x82, 1, "b", "v")), b3)}, corrupt: seg(1), offset: int64(bstart)},
		{name: "rule 23: bit 7 in a snapshot is corrupt",
			files: map[string][]byte{seg(1): cat(hdr(true, 9), bput(3, "a", "A"), put(4, "b", "B"))}, corrupt: seg(1), offset: 16},
		{name: "rule 38: a batch whose first number is not above LastSeq is skipped whole",
			files: map[string][]byte{seg(1): cat(h, put(1, "a", "1"), put(6, "b", "1"), bput(5, "c", "1"), bput(6, "d", "1"), put(7, "e", "1"), put(7, "f", "1"), put(8, "g", "1"))},
			state: mk(8, "a", "1", "b", "1", "f", "1", "g", "1"),
			after: map[string][]byte{seg(1): cat(h, put(1, "a", "1"), put(6, "b", "1"), bput(5, "c", "1"), bput(6, "d", "1"), put(7, "e", "1"), put(7, "f", "1"), put(8, "g", "1"))}, active: seg(1)},
		{name: "rule 38: a batch after a snapshot is applied as a whole",
			files: map[string][]byte{seg(3): cat(hdr(true, 4), put(2, "a", "A")), seg(4): cat(h, bput(4, "x", "stale"), put(5, "y", "stale"), bput(5, "a", "new"), bdel(6, "a"), put(7, "b", "B"))},
			state: mk(7, "b", "B"),
			after: map[string][]byte{seg(3): cat(hdr(true, 4), put(2, "a", "A")), seg(4): cat(h, bput(4, "x", "stale"), put(5, "y", "stale"), bput(5, "a", "new"), bdel(6, "a"), put(7, "b", "B"))}, active: seg(4)},
		{name: "rule 37: an open batch at the end of a lower segment is corrupt at its start",
			files: map[string][]byte{seg(1): cat(h, r1, b1, b2), seg(2): cat(h, put(4, "c", "three"))}, corrupt: seg(1), offset: int64(bstart)},
		{name: "rule 37: a batch torn at the end of the highest of two segments",
			files: map[string][]byte{seg(1): cat(h, r1), seg(2): cat(h, put(2, "q", "Q"), bput(3, "b", "two"), bdel(4, "a"))},
			state: mk(2, "a", "one", "q", "Q"), after: map[string][]byte{seg(1): cat(h, r1), seg(2): cat(h, put(2, "q", "Q"))}, active: seg(2)},
	}
	for _, c := range cases {
		runDirCase(t, c)
	}
}

// ---------------------------------------------------------------------------
// Rules 39 to 41: links between segments.

func TestLinks(t *testing.T) {
	h := hdr(false, 0)
	r1, r2 := put(1, "a", "one"), put(2, "b", "two")
	cases := []dirCase{
		{raw: true, name: "rule 41: a wrong link is corrupt at offset 8",
			files: map[string][]byte{seg(1): cat(h, r1), seg(2): cat(hdrL(crcOf(r1)+1), r2)}, corrupt: seg(2), offset: 8},
		{raw: true, name: "rule 41: link 0 above a segment with records is corrupt",
			files: map[string][]byte{seg(1): cat(h, r1), seg(2): cat(h, r2)}, corrupt: seg(2), offset: 8},
		{raw: true, name: "rule 41: the lowest live segment must have link 0",
			files: map[string][]byte{seg(3): cat(hdrL(77), r1)}, corrupt: seg(3), offset: 8},
		{raw: true, name: "rule 41: the link is checked before the records",
			files: map[string][]byte{seg(1): cat(h, r1), seg(2): cat(hdrL(5), []byte{1, 0, 0, 0, 0, 0, 0, 0})}, corrupt: seg(2), offset: 8},
		{raw: true, name: "rule 41: a segment above a snapshot links to its last record",
			files: map[string][]byte{seg(1): cat(hdr(true, 3), r1, r2), seg(2): cat(hdrL(crcOf(r1)), put(3, "c", "C"))}, corrupt: seg(2), offset: 8},
		{raw: true, name: "rule 41: a segment above an empty snapshot has link 0",
			files: map[string][]byte{seg(1): hdr(true, 3), seg(2): cat(hdrL(3), put(4, "c", "C"))}, corrupt: seg(2), offset: 8},
		{raw: true, name: "rule 41: obsolete segments do not count",
			files: map[string][]byte{seg(1): cat(h, r1), seg(2): cat(hdr(true, 2), r2), seg(3): cat(hdrL(crcOf(r2)), put(3, "c", "C"))},
			state: mk(3, "b", "two", "c", "C"), after: map[string][]byte{seg(2): cat(hdr(true, 2), r2), seg(3): cat(hdrL(crcOf(r2)), put(3, "c", "C"))}, active: seg(3)},
		{raw: true, name: "rule 39: an empty segment passes its link on",
			files: map[string][]byte{seg(1): cat(h, r1), seg(2): hdrL(crcOf(r1)), seg(3): cat(hdrL(crcOf(r1)), r2)},
			state: mk(2, "a", "one", "b", "two"), after: map[string][]byte{seg(1): cat(h, r1), seg(2): hdrL(crcOf(r1)), seg(3): cat(hdrL(crcOf(r1)), r2)}, active: seg(3)},
		{raw: true, name: "rule 39: stale records count",
			files: map[string][]byte{seg(1): cat(h, r1, r2, put(1, "z", "stale")), seg(2): cat(hdrL(crcOf(put(1, "z", "stale"))), put(3, "c", "C"))},
			state: mk(3, "a", "one", "b", "two", "c", "C"), after: map[string][]byte{seg(1): cat(h, r1, r2, put(1, "z", "stale")), seg(2): cat(hdrL(crcOf(put(1, "z", "stale"))), put(3, "c", "C"))}, active: seg(2)},
		{raw: true, name: "rule 40: a re-created torn creation links to the segment below",
			files: map[string][]byte{seg(1): cat(h, r1, r2), seg(2): []byte("WSEG\x01")},
			state: mk(2, "a", "one", "b", "two"), after: map[string][]byte{seg(1): cat(h, r1, r2), seg(2): hdrL(crcOf(r2))}, active: seg(2)},
		{raw: true, name: "rule 40: a new active segment after a snapshot links to it",
			files: map[string][]byte{seg(4): cat(hdr(true, 9), r1, r2)},
			state: mk(9, "a", "one", "b", "two"), after: map[string][]byte{seg(4): cat(hdr(true, 9), r1, r2), seg(5): hdrL(crcOf(r2))}, active: seg(5)},
	}
	for _, c := range cases {
		runDirCase(t, c)
	}
}

// After Open cut a torn end, the next rotation links to what remains.
func TestLinkAfterCut(t *testing.T) {
	h := hdr(false, 0)
	r1, r2 := put(1, "a", "one"), put(2, "b", "two")
	batch := cat(bput(3, "c", "C"), bdel(4, "a"), put(5, "d", "D"))
	stale := put(2, "s", "stale")
	cases := []struct {
		name   string
		file   []byte
		keep   []byte
		chain  uint32
		before st
	}{
		{"a torn single record", cat(h, r1, r2, put(3, "c", "C")[:12]), cat(h, r1, r2), crcOf(r2), mk(2, "a", "one", "b", "two")},
		{"a torn batch", cat(h, r1, r2, batch[:len(batch)-4]), cat(h, r1, r2), crcOf(r2), mk(2, "a", "one", "b", "two")},
		{"a batch without its last record", cat(h, r1, r2, bput(3, "c", "C"), bdel(4, "a")), cat(h, r1, r2), crcOf(r2), mk(2, "a", "one", "b", "two")},
		{"a stale record before a torn one", cat(h, r1, r2, stale, put(3, "c", "C")[:12]), cat(h, r1, r2, stale), crcOf(stale), mk(2, "a", "one", "b", "two")},
		{"everything torn", cat(hdrL(0), put(1, "c", "C")[:12]), hdrL(0), 0, mk(0)},
	}
	for _, c := range cases {
		m := memWith(map[string][]byte{seg(1): c.file})
		l, err := wal.Open("db", wal.Options{FS: m, SegmentSize: int64(len(c.keep))})
		if err != nil {
			t.Fatalf("rule 22/37 with %s: Open returned %v", c.name, err)
		}
		wantState(t, l, c.before, "rules 22, 37 with "+c.name)
		big := strings.Repeat("x", 30)
		seq, err := l.Put("next", []byte(big))
		if err != nil || seq != c.before.last+1 {
			t.Fatalf("with %s: Put = %d, %v", c.name, seq, err)
		}
		want := map[string]string{seg(1): string(c.keep)}
		if len(c.keep) == 16 {
			want[seg(1)] = string(cat(c.keep, put(seq, "next", big)))
		} else {
			want[seg(2)] = string(cat(hdrL(c.chain), put(seq, "next", big)))
		}
		if d := dirDiff(m.dir(), want); d != "" {
			t.Fatalf("rules 39, 40 with %s: after the cut and a rotation: %s", c.name, d)
		}
		l.Close()
		l = mustOpen(t, m, 0)
		l.Close()
	}
}
