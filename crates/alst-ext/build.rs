fn main() {
    // `include_dir!` doesn't track its input; rebuild when a bundled extension changes.
    println!("cargo:rerun-if-changed=../../extensions");
}
