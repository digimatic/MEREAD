//! A minimal scratch-directory helper for the unit tests, so that no dev-dependency has to be
//! justified to `cargo-machete` and `cargo deny` for something this small.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

/// A uniquely named directory under the system temp dir, removed when dropped.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(name: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let path = std::env::temp_dir().join(format!(
            "meread-test-{}-{}-{}",
            name,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));

        // a leftover from a killed run would make the test lie
        fs::remove_dir_all(&path).ok();
        fs::create_dir_all(&path).expect("failed to create temp dir");

        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// Create a directory below the temp dir, parents included. Returns its path.
    pub(crate) fn dir(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(&path).expect("failed to create dir");
        path
    }

    /// Create a file below the temp dir, parents included. Returns its path.
    pub(crate) fn file(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent dir");
        }
        fs::write(&path, contents).expect("failed to write file");
        path
    }

    /// The temp dir as the server sees it, with symlinks such as macos' `/tmp` resolved.
    pub(crate) fn canonical(&self) -> PathBuf {
        self.0
            .canonicalize()
            .expect("failed to canonicalize temp dir")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}
