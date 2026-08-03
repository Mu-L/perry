use std::fs;
use std::path::{Path, PathBuf};

/// Recursively gather every JavaScript file under a Next.js server bundle.
pub(super) fn collect_js_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_js_files_recursive(&path, out);
        } else if file_type.is_file() && path.extension().and_then(|e| e.to_str()) == Some("js") {
            out.push(path);
        }
    }
}

/// Return whether a path belongs to a standalone `.next/server/**` bundle.
pub(crate) fn is_nextjs_runtime_module(path: &Path) -> bool {
    let comps: Vec<&std::ffi::OsStr> = path.components().map(|c| c.as_os_str()).collect();
    comps
        .windows(2)
        .any(|w| w[0] == std::ffi::OsStr::new(".next") && w[1] == std::ffi::OsStr::new("server"))
}
