//! The interface in `ui-dist` is embedded with `include_dir!`, which cannot
//! tell Cargo about the files it reads; this makes a rebuilt interface
//! rebuild the crate.
fn main() {
    println!("cargo:rerun-if-changed=ui-dist");
}
