use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest.join("../../frontend/dist");
    println!("cargo:rerun-if-changed={}", dist.display());

    let mut files = Vec::new();
    if dist.is_dir() {
        collect_files(&dist, &dist, &mut files).unwrap();
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("embedded_frontend.rs");
    let mut source = String::from("pub static FILES: &[EmbeddedFile] = &[\n");
    for (path, absolute) in files {
        source.push_str(&format!(
            "    EmbeddedFile {{ path: {path:?}, bytes: include_bytes!({absolute:?}) }},\n"
        ));
    }
    source.push_str("];\n");
    fs::write(output, source).unwrap();
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, String)>,
) -> std::io::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else if path.is_file() {
            println!("cargo:rerun-if-changed={}", path.display());
            let relative = path.strip_prefix(root).unwrap();
            files.push((
                relative.to_string_lossy().replace('\\', "/"),
                path.to_string_lossy().into_owned(),
            ));
        }
    }
    Ok(())
}
