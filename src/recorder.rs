//! Opt-in session recording to JSONL with hard size and retention limits.
//!
//! Nothing records unless the user enabled `[recording]` in the config AND
//! started a session (Ctrl+R / palette). Lines are the same sanitized, redacted
//! documents as `snapshot --json`. Writing happens on its own thread through a
//! bounded queue, so a slow disk can never stall the UI (lines are dropped, and
//! counted, instead).
//!
//! Retention only ever touches files named `systui-*.jsonl` directly inside the
//! recordings directory.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

const QUEUE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// A new file is started before a file would exceed this size.
    pub max_file_bytes: u64,
    /// Keep at most this many recording files.
    pub max_files: usize,
    /// Keep at most this many bytes in total.
    pub max_total_bytes: u64,
    pub max_age: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

pub fn is_recording_name(name: &str) -> bool {
    name.starts_with("systui-") && name.ends_with(".jsonl") && !name.contains(['/', '\\'])
}

/// Recording files directly inside `dir` (no recursion, symlinks ignored).
pub fn list_recordings(dir: &Path) -> Vec<FileInfo> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_file() || !is_recording_name(&e.file_name().to_string_lossy()) {
            continue;
        }
        if let Ok(m) = e.metadata() {
            v.push(FileInfo {
                path: e.path(),
                size: m.len(),
                modified: m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    v
}

/// Which files to delete. Oldest go first; `keep` (the active file) is never
/// returned but still counts toward the limits.
pub fn plan_retention(
    files: &[FileInfo],
    l: &Limits,
    now: SystemTime,
    keep: &Path,
) -> Vec<PathBuf> {
    let mut sorted: Vec<&FileInfo> = files.iter().collect();
    sorted.sort_by(|a, b| a.modified.cmp(&b.modified).then(a.path.cmp(&b.path)));
    let mut delete = Vec::new();
    let mut remaining: Vec<&FileInfo> = Vec::new();
    for f in sorted {
        let too_old = now
            .duration_since(f.modified)
            .is_ok_and(|age| age > l.max_age);
        if too_old && f.path != keep {
            delete.push(f.path.clone());
        } else {
            remaining.push(f);
        }
    }
    let mut total: u64 = remaining.iter().map(|f| f.size).sum();
    let mut count = remaining.len();
    for f in remaining {
        if count <= l.max_files && total <= l.max_total_bytes {
            break;
        }
        if f.path == keep {
            continue;
        }
        delete.push(f.path.clone());
        count -= 1;
        total = total.saturating_sub(f.size);
    }
    delete
}

fn apply_retention(dir: &Path, l: &Limits, keep: &Path) {
    let files = list_recordings(dir);
    for p in plan_retention(&files, l, SystemTime::now(), keep) {
        if p.parent() == Some(dir) {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[derive(Default)]
pub struct Stats {
    pub lines: AtomicU64,
    pub bytes: AtomicU64,
    pub dropped: AtomicU64,
    pub files: AtomicU64,
    pub error: Mutex<Option<String>>,
    pub current: Mutex<Option<PathBuf>>,
}

fn set_error(st: &Stats, msg: String) {
    if let Ok(mut e) = st.error.lock() {
        *e = Some(msg);
    }
}

pub struct Recorder {
    tx: Option<SyncSender<String>>,
    pub stats: Arc<Stats>,
    join: Option<JoinHandle<()>>,
    pub dir: PathBuf,
}

/// `systui-YYYYMMDD-HHMMSS` from a UTC timestamp string `YYYY-MM-DDTHH:MM:SSZ`.
fn file_stem(utc: &str) -> String {
    let d: String = utc.chars().filter(|c| c.is_ascii_digit()).collect();
    format!(
        "systui-{}-{}",
        &d[..8.min(d.len())],
        d.get(8..14).unwrap_or("000000")
    )
}

fn open_new(dir: &Path, stem: &str) -> std::io::Result<(std::fs::File, PathBuf)> {
    for n in 0..1000 {
        let name = if n == 0 {
            format!("{stem}.jsonl")
        } else {
            format!("{stem}-{n}.jsonl")
        };
        let p = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(f) => return Ok((f, p)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(
        "could not find a free recording file name",
    ))
}

impl Recorder {
    /// Create the directory and the first file immediately so problems surface now.
    pub fn start(dir: &Path, limits: Limits, utc_now: &str) -> std::io::Result<Recorder> {
        std::fs::create_dir_all(dir)?;
        let stem = file_stem(utc_now);
        let (file, path) = open_new(dir, &stem)?;
        apply_retention(dir, &limits, &path);
        let stats = Arc::new(Stats::default());
        stats.files.store(1, Ordering::Relaxed);
        if let Ok(mut c) = stats.current.lock() {
            *c = Some(path.clone());
        }
        let (tx, rx) = mpsc::sync_channel::<String>(QUEUE);
        let st = stats.clone();
        let d = dir.to_path_buf();
        let join = std::thread::Builder::new()
            .name("systui-recorder".into())
            .spawn(move || {
                let (mut file, mut path, mut size) = (file, path, 0u64);
                while let Ok(line) = rx.recv() {
                    let need = line.len() as u64 + 1;
                    if need > limits.max_file_bytes {
                        st.dropped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    if size + need > limits.max_file_bytes {
                        let stem = file_stem(&crate::export::utc_string(
                            SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0),
                        ));
                        match open_new(&d, &stem) {
                            Ok((f, p)) => {
                                (file, path, size) = (f, p, 0);
                                st.files.fetch_add(1, Ordering::Relaxed);
                                if let Ok(mut c) = st.current.lock() {
                                    *c = Some(path.clone());
                                }
                                apply_retention(&d, &limits, &path);
                            }
                            Err(e) => {
                                set_error(&st, format!("cannot rotate recording: {e}"));
                                return;
                            }
                        }
                    }
                    if let Err(e) = file
                        .write_all(line.as_bytes())
                        .and_then(|_| file.write_all(b"\n"))
                        .and_then(|_| file.flush())
                    {
                        set_error(&st, format!("write to {} failed: {e}", path.display()));
                        return;
                    }
                    size += need;
                    st.lines.fetch_add(1, Ordering::Relaxed);
                    st.bytes.fetch_add(need, Ordering::Relaxed);
                }
            })
            .map_err(std::io::Error::other)?;
        Ok(Recorder {
            tx: Some(tx),
            stats,
            join: Some(join),
            dir: dir.to_path_buf(),
        })
    }

    /// Queue one JSON line. Returns false (and counts a drop) if the writer is behind or gone.
    pub fn record(&self, line: String) -> bool {
        let Some(tx) = &self.tx else { return false };
        match tx.try_send(line) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn error(&self) -> Option<String> {
        self.stats.error.lock().ok().and_then(|e| e.clone())
    }

    pub fn current_file(&self) -> Option<PathBuf> {
        self.stats.current.lock().ok().and_then(|c| c.clone())
    }

    /// Flush and stop; returns the final stats snapshot (lines, bytes, dropped).
    pub fn stop(mut self) -> (u64, u64, u64) {
        drop(self.tx.take());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        (
            self.stats.lines.load(Ordering::Relaxed),
            self.stats.bytes.load(Ordering::Relaxed),
            self.stats.dropped.load(Ordering::Relaxed),
        )
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fi(name: &str, size: u64, age_secs: u64, now: SystemTime) -> FileInfo {
        FileInfo {
            path: PathBuf::from(format!("/rec/{name}")),
            size,
            modified: now - Duration::from_secs(age_secs),
        }
    }

    fn limits() -> Limits {
        Limits {
            max_file_bytes: 1000,
            max_files: 3,
            max_total_bytes: 10_000,
            max_age: Duration::from_secs(1000),
        }
    }

    #[test]
    fn names_are_strictly_matched() {
        assert!(is_recording_name("systui-20261102-142032.jsonl"));
        assert!(!is_recording_name("notes.jsonl"));
        assert!(!is_recording_name("systui-x.json"));
        assert!(!is_recording_name("systui-../evil.jsonl"));
    }

    #[test]
    fn age_limit_deletes_old_files_but_never_the_active_one() {
        let now = SystemTime::now();
        let files = vec![
            fi("a", 10, 5000, now),
            fi("b", 10, 2000, now),
            fi("c", 10, 10, now),
        ];
        let del = plan_retention(&files, &limits(), now, Path::new("/rec/b"));
        assert_eq!(
            del,
            [PathBuf::from("/rec/a")],
            "b is too old but is the active file"
        );
    }

    #[test]
    fn count_limit_removes_the_oldest_first() {
        let now = SystemTime::now();
        let files: Vec<_> = (0..6)
            .map(|i| fi(&format!("f{i}"), 10, 100 - i * 10, now))
            .collect();
        // f0 is oldest ... f5 newest
        let del = plan_retention(&files, &limits(), now, Path::new("/rec/f5"));
        assert_eq!(
            del,
            [
                PathBuf::from("/rec/f0"),
                PathBuf::from("/rec/f1"),
                PathBuf::from("/rec/f2")
            ]
        );
    }

    #[test]
    fn total_size_limit_applies_and_counts_the_active_file() {
        let now = SystemTime::now();
        let files = vec![
            fi("a", 6000, 300, now),
            fi("b", 6000, 200, now),
            fi("c", 3000, 100, now),
        ];
        let del = plan_retention(&files, &limits(), now, Path::new("/rec/c"));
        assert_eq!(
            del,
            [PathBuf::from("/rec/a")],
            "15 000 > 10 000: drop oldest until 9 000"
        );
        // active file alone over the limit: nothing else to delete, and it is never deleted
        let big = vec![fi("c", 50_000, 1, now)];
        assert!(plan_retention(&big, &limits(), now, Path::new("/rec/c")).is_empty());
    }

    #[test]
    fn nothing_to_do_within_limits() {
        let now = SystemTime::now();
        let files = vec![fi("a", 10, 10, now), fi("b", 10, 5, now)];
        assert!(plan_retention(&files, &limits(), now, Path::new("/rec/b")).is_empty());
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("systui-rec-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn writes_valid_lines_and_reports_stats() {
        let dir = tmp("write");
        let r = Recorder::start(
            &dir,
            Limits {
                max_file_bytes: 1 << 20,
                max_files: 5,
                max_total_bytes: 1 << 24,
                max_age: Duration::from_secs(3600),
            },
            "2026-11-02T14:20:32Z",
        )
        .unwrap();
        assert!(r
            .current_file()
            .unwrap()
            .ends_with("systui-20261102-142032.jsonl"));
        for i in 0..20 {
            assert!(r.record(format!("{{\"n\":{i}}}")));
        }
        let (lines, bytes, dropped) = r.stop();
        assert_eq!((lines, dropped), (20, 0));
        let files = list_recordings(&dir);
        assert_eq!(files.len(), 1);
        let text = std::fs::read_to_string(&files[0].path).unwrap();
        assert_eq!(text.lines().count(), 20);
        assert_eq!(bytes, text.len() as u64);
        for l in text.lines() {
            serde_json::from_str::<serde_json::Value>(l).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_at_the_file_size_limit_and_enforces_retention() {
        let dir = tmp("rotate");
        let lim = Limits {
            max_file_bytes: 200,
            max_files: 3,
            max_total_bytes: 1 << 20,
            max_age: Duration::from_secs(3600),
        };
        let r = Recorder::start(&dir, lim, "2026-11-02T14:20:32Z").unwrap();
        let line = format!("{{\"pad\":\"{}\"}}", "x".repeat(80)); // ~90 bytes: 2 per file
        for _ in 0..30 {
            r.record(line.clone());
            std::thread::sleep(Duration::from_millis(30)); // keep the bounded queue from filling
        }
        let (lines, _, dropped) = r.stop();
        assert_eq!(lines + dropped, 30);
        let files = list_recordings(&dir);
        assert!(
            files.len() <= 3,
            "retention keeps at most 3 files, found {}",
            files.len()
        );
        assert!(
            files.iter().all(|f| f.size <= 200),
            "no file exceeds the limit: {files:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_lines_are_dropped_not_written() {
        let dir = tmp("big");
        let lim = Limits {
            max_file_bytes: 100,
            max_files: 3,
            max_total_bytes: 1 << 20,
            max_age: Duration::from_secs(3600),
        };
        let r = Recorder::start(&dir, lim, "2026-11-02T14:20:32Z").unwrap();
        r.record("y".repeat(500));
        r.record("{\"ok\":1}".into());
        let (lines, _, dropped) = r.stop();
        assert_eq!((lines, dropped), (1, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_never_touches_foreign_files() {
        let dir = tmp("foreign");
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["notes.txt", "data.jsonl", "systui-backup.zip"] {
            std::fs::write(dir.join(n), "keep me").unwrap();
        }
        let lim = Limits {
            max_file_bytes: 1 << 20,
            max_files: 1,
            max_total_bytes: 1,
            max_age: Duration::from_secs(0),
        };
        let r = Recorder::start(&dir, lim, "2026-11-02T14:20:32Z").unwrap();
        r.stop();
        for n in ["notes.txt", "data.jsonl", "systui-backup.zip"] {
            assert!(dir.join(n).exists(), "{n} must survive retention");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_failure_is_reported_immediately() {
        let f = std::env::temp_dir().join(format!("systui-rec-file-{}", std::process::id()));
        std::fs::write(&f, "i am a file, not a directory").unwrap();
        let err = Recorder::start(&f.join("sub"), limits(), "2026-11-02T14:20:32Z");
        assert!(err.is_err());
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn same_second_start_does_not_overwrite_an_existing_file() {
        let dir = tmp("collide");
        let a = Recorder::start(&dir, limits(), "2026-11-02T14:20:32Z").unwrap();
        let b = Recorder::start(&dir, limits(), "2026-11-02T14:20:32Z").unwrap();
        assert_ne!(a.current_file(), b.current_file());
        a.stop();
        b.stop();
        assert_eq!(list_recordings(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
