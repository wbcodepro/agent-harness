from textutil import shout, slugify

assert shout("hi") == "HI!"
assert slugify("Hello, World!") == "hello-world"
assert slugify("  Rust & WASM -- 2026 ") == "rust-wasm-2026"
assert slugify("already-ok") == "already-ok"
print("ok")
