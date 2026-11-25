//! Every run's output in a log file: `<config dir>/logs/shuppatsu-<UTC time, to the ms>.log` (with a
//! launcher's `SHUPPATSU_HOME`, `{data}/logs`), the newest [`KEEP`] kept. A launcher shows the
//! newest one with a crash and sends it with the player's report (RecompLauncher's
//! `folders.logs`), so a player never has to find a terminal.
//!
//! The game writes with `eprintln!`/`println!`. Here stdout and stderr are pointed at pipes
//! whose reader threads copy everything to the original stream (the terminal, the launcher's
//! pipe) and to the file, so nothing else changes. [`finish`] puts the streams back and waits
//! for the copies, so the last lines (the error `main` prints) reach the file before exit.
//!
//! On Windows without a usable stderr (started from Explorer) `win_console` points both
//! streams at the file directly, the same path from [`new_path`]. Elsewhere and otherwise the
//! tee below is used. The only `unsafe` is the descriptor/handle FFI, as in `win_console`.
#![allow(unsafe_code)]

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Log files kept, the new one included.
pub const KEEP: usize = 5;
const PREFIX: &str = "shuppatsu-";

/// The folder of the logs.
pub fn dir() -> Option<PathBuf> {
    Some(crate::save::config_dir()?.join("logs"))
}

/// A new log file's path (the folder made, the oldest logs beyond [`KEEP`] removed).
pub fn new_path() -> Option<PathBuf> {
    let dir = dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    prune(&dir, KEEP - 1);
    // Milliseconds: a launcher starts the game right after its setup step, often in the same
    // second, and each keeps its own log.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let name = format!(
        "{PREFIX}{}-{:03}.log",
        stamp(now.as_secs()),
        now.subsec_millis()
    );
    Some(dir.join(name))
}

/// Keeps the newest `keep` logs in `dir` (by name: the stamp sorts by time).
fn prune(dir: &std::path::Path, keep: usize) {
    let mut logs: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(".log"))
        })
        .collect();
    logs.sort();
    let extra = logs.len().saturating_sub(keep);
    for old in &logs[..extra] {
        let _ = std::fs::remove_file(old);
    }
}

/// `YYYYMMDD-HHMMSS` (UTC) for seconds since 1970.
fn stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// The running copy: the reader threads and how to put the streams back.
struct Tee {
    threads: Vec<JoinHandle<()>>,
    restore: Vec<Restore>,
}

static TEE: Mutex<Option<Tee>> = Mutex::new(None);

/// Starts copying stdout and stderr to a new log file. Does nothing when the streams already
/// go to a file of ours (`win_console`) or no log folder can be made.
pub fn start() {
    let Some(path) = new_path() else {
        return;
    };
    let Ok(file) = File::create(&path) else {
        return;
    };
    let file = Arc::new(Mutex::new(file));
    let mut tee = Tee {
        threads: Vec::new(),
        restore: Vec::new(),
    };
    for stream in [Stream::Out, Stream::Err] {
        let Ok((reader, writer)) = std::io::pipe() else {
            break;
        };
        let Some((original, restore)) = redirect(stream, writer) else {
            break;
        };
        tee.restore.push(restore);
        let file = file.clone();
        tee.threads
            .push(std::thread::spawn(move || copy(reader, original, &file)));
    }
    // A panic on the main thread ends the process without `main`'s finish: finish here, after
    // the message is printed.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        if std::thread::current().name() == Some("main") {
            finish();
        }
    }));
    *TEE.lock().unwrap_or_else(|e| e.into_inner()) = Some(tee);
}

/// Puts stdout and stderr back and waits until everything written so far is in the file.
pub fn finish() {
    let Some(tee) = TEE.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    for r in tee.restore {
        r.restore();
    }
    for t in tee.threads {
        let _ = t.join();
    }
}

fn copy(mut reader: std::io::PipeReader, mut original: File, file: &Mutex<File>) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let _ = original.write_all(&buf[..n]);
                let _ = file
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .write_all(&buf[..n]);
            }
        }
    }
    let _ = original.flush();
    // `original` is a duplicate of the stream's first target: closing it leaves that open.
}

#[derive(Clone, Copy)]
enum Stream {
    Out,
    Err,
}

#[cfg(unix)]
mod sys {
    use super::Stream;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};

    unsafe extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(old: i32, new: i32) -> i32;
        fn close(fd: i32) -> i32;
    }

    fn fd(s: Stream) -> RawFd {
        match s {
            Stream::Out => 1,
            Stream::Err => 2,
        }
    }

    /// Restores one stream from a saved duplicate.
    pub struct Restore {
        stream: Stream,
        saved: RawFd,
    }

    impl Restore {
        pub fn restore(self) {
            // SAFETY: `saved` is a descriptor we duplicated and still own; dup2 onto 1/2 closes
            // the pipe's writing end there, and closing `saved` afterwards releases our copy.
            unsafe {
                dup2(self.saved, fd(self.stream));
                close(self.saved);
            }
        }
    }

    /// Points `stream` at `writer`; returns a duplicate of its old target to copy to.
    pub fn redirect(stream: Stream, writer: std::io::PipeWriter) -> Option<(File, Restore)> {
        let target = fd(stream);
        // SAFETY: plain descriptor calls on 1/2 and on descriptors we own; every result is
        // checked, and on failure nothing is left changed.
        unsafe {
            let saved = dup(target);
            if saved < 0 {
                return None;
            }
            let copy = dup(target);
            if copy < 0 || dup2(writer.as_raw_fd(), target) < 0 {
                close(saved);
                if copy >= 0 {
                    close(copy);
                }
                return None;
            }
            // `writer` drops here: 1/2 now hold the pipe's writing end.
            Some((File::from_raw_fd(copy), Restore { stream, saved }))
        }
    }
}

#[cfg(windows)]
mod sys {
    use super::Stream;
    use std::ffi::c_void;
    use std::fs::File;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const DUPLICATE_SAME_ACCESS: u32 = 2;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetStdHandle(std_handle: u32, handle: *mut c_void) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
        fn DuplicateHandle(
            src_process: *mut c_void,
            src: *mut c_void,
            target_process: *mut c_void,
            target: *mut *mut c_void,
            access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
    }

    fn id(s: Stream) -> u32 {
        match s {
            Stream::Out => STD_OUTPUT_HANDLE,
            Stream::Err => STD_ERROR_HANDLE,
        }
    }

    pub struct Restore {
        stream: Stream,
        saved: *mut c_void,
        writer: OwnedHandle,
    }

    // SAFETY: the handles are plain kernel object handles, usable from any thread.
    unsafe impl Send for Restore {}

    impl Restore {
        pub fn restore(self) {
            // SAFETY: `saved` is the stream's original handle, still open; setting it back and
            // then dropping our pipe writer closes the pipe so the reader sees its end.
            unsafe {
                SetStdHandle(id(self.stream), self.saved);
            }
            drop(self.writer);
        }
    }

    pub fn redirect(stream: Stream, writer: std::io::PipeWriter) -> Option<(File, Restore)> {
        let saved = match stream {
            Stream::Out => std::io::stdout().as_raw_handle(),
            Stream::Err => std::io::stderr().as_raw_handle(),
        };
        if saved.is_null() || saved as isize == -1 {
            return None;
        }
        let mut copy: *mut c_void = std::ptr::null_mut();
        // SAFETY: duplicating a valid handle of our own process into `copy`; checked below.
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                saved,
                GetCurrentProcess(),
                &mut copy,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            return None;
        }
        let writer: OwnedHandle = writer.into();
        // SAFETY: `writer` stays open in `Restore` until the stream is set back.
        if unsafe { SetStdHandle(id(stream), writer.as_raw_handle()) } == 0 {
            // SAFETY: `copy` is ours and unused.
            drop(unsafe { OwnedHandle::from_raw_handle(copy) });
            return None;
        }
        // SAFETY: `copy` is a fresh handle we own; the File closes it, not the original.
        let original = unsafe { File::from_raw_handle(copy) };
        Some((
            original,
            Restore {
                stream,
                saved,
                writer,
            },
        ))
    }
}

use sys::{Restore, redirect};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_utc_and_sort_by_time() {
        assert_eq!(stamp(0), "19700101-000000");
        // 2026-11-25 23:59:59 UTC and the second after.
        assert_eq!(stamp(1_795_651_199), "20261125-235959");
        assert_eq!(stamp(1_795_651_200), "20261126-000000");
        assert_eq!(stamp(951_782_400), "20000229-000000"); // a leap day
        assert!(stamp(1_000) < stamp(100_000));
    }

    #[test]
    fn keeps_the_newest_logs() {
        let tmp = std::env::temp_dir().join(format!("shuppatsu-logs-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        for i in 0..8 {
            std::fs::write(tmp.join(format!("{PREFIX}2026010{i}-000000.log")), b"").unwrap();
        }
        std::fs::write(tmp.join("other.txt"), b"").unwrap();
        prune(&tmp, 4);
        let mut left: Vec<_> = std::fs::read_dir(&tmp)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "other.txt",
                "shuppatsu-20260104-000000.log",
                "shuppatsu-20260105-000000.log",
                "shuppatsu-20260106-000000.log",
                "shuppatsu-20260107-000000.log",
            ]
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
