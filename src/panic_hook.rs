//! Persist panic payload + backtrace to `~/Library/Logs/Operator/` before the
//! process unwinds. Without this, a UI-thread panic produces a bare SIGABRT
//! in macOS crash reports with no way to tell what actually went wrong.

use std::backtrace::Backtrace;
use std::fs;
use std::io::Write;
use std::panic;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn install() {
    let log_dir = log_dir();
    if let Some(dir) = &log_dir {
        let _ = fs::create_dir_all(dir);
    }

    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let backtrace = Backtrace::force_capture();
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".into());
        let payload = payload_str(info.payload());

        let report = format!(
            "Operator v{version} panic\n\
             time_unix_nanos: {nanos}\n\
             thread: {thread_name}\n\
             location: {location}\n\
             message: {payload}\n\
             \n\
             backtrace:\n{backtrace}\n",
            version = env!("CARGO_PKG_VERSION"),
            nanos = unix_nanos(),
        );

        log::error!("panic on thread {thread_name} at {location}: {payload}");

        if let Some(dir) = &log_dir {
            let path = dir.join(format!("panic-{}-{}.log", unix_nanos(), std::process::id()));
            if let Ok(mut file) = fs::File::create(&path) {
                let _ = file.write_all(report.as_bytes());
            }
        }

        previous(info);
    }));
}

fn payload_str(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "<non-string panic payload>"
    }
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library/Logs/Operator"))
}
