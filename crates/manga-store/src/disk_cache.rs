use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use bytes::Bytes;
use manga_core::ByteCache;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

/// Content cache keyed by `sha256(key)`, two-level fan-out directories.
///
/// - writes are atomic (temp file + rename), so a crash never leaves a
///   truncated image behind
/// - total size is capped; when exceeded, least-recently-used files (by
///   mtime, refreshed on hit) are evicted down to 80% of the cap
pub struct DiskCache {
    root: PathBuf,
    cap_bytes: u64,
    approx_size: AtomicU64,
    evict_lock: Mutex<()>,
}

impl DiskCache {
    pub fn open(root: impl Into<PathBuf>, cap_bytes: u64) -> std::io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        let size = dir_size(&root);
        Ok(Self { root, cap_bytes, approx_size: AtomicU64::new(size), evict_lock: Mutex::new(()) })
    }

    pub fn size(&self) -> u64 {
        self.approx_size.load(Ordering::Relaxed)
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let hex = format!("{:x}", Sha256::digest(key.as_bytes()));
        self.root.join(&hex[..2]).join(&hex[2..])
    }

    fn evict_if_needed(&self) {
        if self.size() <= self.cap_bytes {
            return;
        }
        let Some(_g) = self.evict_lock.try_lock() else { return };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        for sub in fs::read_dir(&self.root).into_iter().flatten().flatten() {
            for f in fs::read_dir(sub.path()).into_iter().flatten().flatten() {
                if let Ok(m) = f.metadata()
                    && m.is_file()
                {
                    files.push((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), f.path()));
                }
            }
        }
        files.sort_by_key(|f| f.0);
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        let target = self.cap_bytes / 10 * 8;
        for (_, len, path) in files {
            if total <= target {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
        self.approx_size.store(total, Ordering::Relaxed);
    }
}

impl ByteCache for DiskCache {
    fn get(&self, key: &str) -> Option<Bytes> {
        let p = self.path_for(key);
        let data = fs::read(&p).ok()?;
        // Touch for LRU. Failure is harmless.
        if let Ok(f) = fs::File::options().write(true).open(&p) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(data.into())
    }

    fn put(&self, key: &str, data: &[u8]) {
        let p = self.path_for(key);
        let Some(dir) = p.parent() else { return };
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        let tmp = dir.join(format!("{TMP_PREFIX}{}-{:?}", std::process::id(), std::thread::current().id()));
        // Overwriting the same key replaces its bytes; account only the delta.
        let old_len = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        // No fsync: losing a cache entry on power loss is fine, and the
        // rename already prevents a truncated file from being visible.
        let ok = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(data)?;
            fs::rename(&tmp, &p)
        })();
        if ok.is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }
        self.approx_size.fetch_add(data.len() as u64, Ordering::Relaxed);
        let _ = self.approx_size.try_update(Ordering::Relaxed, Ordering::Relaxed, |s| Some(s.saturating_sub(old_len)));
        self.evict_if_needed();
    }
}

const TMP_PREFIX: &str = ".tmp-";

/// Total size of cached entries; removes temp files left by a crash.
fn dir_size(root: &Path) -> u64 {
    let mut total = 0;
    for sub in fs::read_dir(root).into_iter().flatten().flatten() {
        for f in fs::read_dir(sub.path()).into_iter().flatten().flatten() {
            if f.file_name().to_string_lossy().starts_with(TMP_PREFIX) {
                let _ = fs::remove_file(f.path());
                continue;
            }
            total += f.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_evict() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::open(dir.path(), 1000).unwrap();
        c.put("a", &[1u8; 400]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        c.put("b", &[2u8; 400]);
        assert_eq!(c.get("a").unwrap().len(), 400);
        std::thread::sleep(std::time::Duration::from_millis(20));
        c.put("c", &[3u8; 400]); // 1200 > 1000: evict LRU ("b", since "a" was touched)
        assert!(c.get("b").is_none());
        assert!(c.get("a").is_some());
        assert!(c.get("c").is_some());
        assert!(c.size() <= 1000);
    }

    #[test]
    fn overwrite_counts_delta_and_tmp_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::open(dir.path(), 10_000).unwrap();
        c.put("a", &[1u8; 300]);
        c.put("a", &[1u8; 300]);
        assert_eq!(c.size(), 300);
        let stray = dir.path().join("ab");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join(".tmp-1-x"), [0u8; 50]).unwrap();
        let reopened = DiskCache::open(dir.path(), 10_000).unwrap();
        assert_eq!(reopened.size(), 300);
        assert!(!stray.join(".tmp-1-x").exists());
    }
}
