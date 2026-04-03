use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Tracks which files have been read in the current session and their
/// modification time at the time of reading. Used by file_edit to detect
/// stale reads before writing.
#[derive(Default, Clone)]
pub struct FileReadState {
    entries: HashMap<PathBuf, FileReadRecord>,
}

#[derive(Clone, Debug)]
pub struct FileReadRecord {
    pub mtime: SystemTime,
    pub was_full_read: bool,
}

impl FileReadState {
    pub fn record_read(&mut self, path: &Path, mtime: SystemTime, full: bool) {
        self.entries.insert(
            path.to_path_buf(),
            FileReadRecord { mtime, was_full_read: full },
        );
    }

    pub fn get(&self, path: &Path) -> Option<&FileReadRecord> {
        self.entries.get(path)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_retrieve() {
        let mut state = FileReadState::default();
        let path = PathBuf::from("/tmp/test.rs");
        let mtime = SystemTime::now();
        state.record_read(&path, mtime, true);
        let record = state.get(&path).unwrap();
        assert!(record.was_full_read);
        assert_eq!(record.mtime, mtime);
    }

    #[test]
    fn test_missing_path_returns_none() {
        let state = FileReadState::default();
        assert!(state.get(Path::new("/tmp/never.rs")).is_none());
    }

    #[test]
    fn test_clear_removes_all() {
        let mut state = FileReadState::default();
        state.record_read(Path::new("/tmp/a.rs"), SystemTime::now(), true);
        state.clear();
        assert!(state.get(Path::new("/tmp/a.rs")).is_none());
    }
}
