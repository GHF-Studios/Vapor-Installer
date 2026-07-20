//! App-local path conventions.

use crate::fsutil::{executable, is_executable};
use std::path::{Path, PathBuf};

pub(crate) fn basic_directories() -> Vec<PathBuf> {
    [
        ".vapor/state",
        ".vapor/state/installer",
        ".vapor/logs",
        ".vapor/diagnostics/runs",
        ".vapor/downloads",
        "content/cache/packages",
        "content/installed",
        "content/workshop/downloads",
        "tools",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

pub(crate) fn steam_executable(app_root: &Path) -> PathBuf {
    let directory = app_root.join("tools/steamcmd");
    steam_candidates(&directory)
        .into_iter()
        .find(|path| is_executable(path))
        .unwrap_or_else(|| directory.join(executable("steamcmd")))
}

pub(crate) fn steam_candidates(directory: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![directory.join("steamcmd.exe")]
    } else {
        vec![directory.join("steamcmd"), directory.join("steamcmd.sh")]
    }
}

pub(crate) fn zig_executable(app_root: &Path) -> PathBuf {
    app_root.join("tools/zig").join(executable("zig"))
}

pub(crate) fn llvm_mingw_root(app_root: &Path) -> PathBuf {
    app_root.join("tools/llvm-mingw")
}

pub(crate) fn llvm_mingw_bin(app_root: &Path) -> PathBuf {
    llvm_mingw_root(app_root).join("bin")
}

pub(crate) fn cross_linker_path(app_root: &Path, target: &str) -> PathBuf {
    app_root.join("tools/cross/bin").join(if cfg!(windows) {
        format!("{target}-zig-cc.cmd")
    } else {
        format!("{target}-zig-cc")
    })
}
