use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

fn start() -> Instant {
    static S: OnceLock<Instant> = OnceLock::new();
    *S.get_or_init(Instant::now)
}

static SINK: Mutex<Option<File>> = Mutex::new(None);
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());
static DEFERRED: AtomicBool = AtomicBool::new(false);

/// No file I/O is allowed under the loader lock.
pub(crate) fn defer(on: bool) {
    DEFERRED.store(on, Ordering::Release);
}

/// Writes out everything buffered while [`defer`] was on.
pub(crate) fn flush_deferred() {
    let lines = match PENDING.lock() {
        Ok(mut p) => core::mem::take(&mut *p),
        Err(_) => return,
    };
    for line in &lines {
        emit(line);
    }
}

pub(crate) fn write(msg: &str) {
    let ms = start().elapsed().as_millis();
    let line = format!("[{ms:>6}ms] {msg}");
    if DEFERRED.load(Ordering::Acquire)
        && let Ok(mut pending) = PENDING.lock()
    {
        pending.push(line);
        return;
    }
    emit(&line);
}

fn emit(line: &str) {
    let Ok(mut sink) = SINK.lock() else { return };
    if sink.is_none() {
        *sink = open_log();
    }
    if let Some(file) = sink.as_mut() {
        let _ = writeln!(file, "{line}");
    }
}

fn open_log() -> Option<File> {
    let path = crate::dll_dir()?.join("chronostasis.log");
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .ok()?;
    let _ = file.write_all(b"=== chronostasis session ===\n");
    Some(file)
}

macro_rules! flog {
    ($($arg:tt)*) => { $crate::log::write(&format!($($arg)*)) };
}
pub(crate) use flog;
