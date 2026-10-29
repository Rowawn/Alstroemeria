use std::io::Write;
use std::path::{Path, PathBuf};

pub fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("alst-pack-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, base, out);
        } else {
            out.push((p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(&p).unwrap()));
        }
    }
}

/// Zip a fixture folder (hand-made manifest and items) into a `.alpack`.
pub fn zip_fixture(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    let mut files = Vec::new();
    walk(&base, &base, &mut files);
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    zip_files(name, &refs)
}

pub fn zip_files(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let path = tmp(&format!("{name}.alpack"));
    let mut w = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    for (n, d) in files {
        w.start_file(*n, opts).unwrap();
        w.write_all(d).unwrap();
    }
    w.finish().unwrap();
    path
}

