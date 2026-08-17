//! Redacted file tracing.
//!
//! Diagnostics go to a file outside the alternate screen, never to the
//! terminal. Every line is scrubbed against the configured secrets before a
//! single byte reaches disk, so an accidental caption can never spill the API
//! key. Tracing is best-effort: a poisoned lock, missing directory, or failed
//! write must never take the application down.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A cloneable handle to one append-only, redacted trace file.
#[derive(Clone)]
pub struct FileLog {
    inner: Arc<Mutex<Writer>>,
}

struct Writer {
    file: BufWriter<File>,
    secrets: Vec<String>,
}

impl FileLog {
    /// Open (or append to) `path`. Every written line has `secrets`
    /// occurrences replaced before it is flushed.
    pub fn append_at(path: &Path, secrets: Vec<String>) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Writer {
                file: BufWriter::new(file),
                secrets,
            })),
        })
    }

    /// Write one timestamped, redacted diagnostic line.
    pub fn trace(&self, level: &str, message: &str) {
        let Ok(mut writer) = self.inner.lock() else {
            return;
        };
        let line = format!("{} [{level}] {message}\n", timestamp());
        let line = redact(&line, &writer.secrets);
        let _ = writer.file.write_all(line.as_bytes());
        let _ = writer.file.flush();
    }

    /// Convenience wrapper for informational events.
    pub fn info(&self, message: &str) {
        self.trace("info", message);
    }
}

/// Scrub every configured secret occurrence from `text`.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        if result.contains(secret.as_str()) {
            result = result.replace(secret.as_str(), "<redacted>");
        }
    }
    result
}

fn timestamp() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_replaces_every_secret_occurrence_and_ignores_empty_secrets() {
        let secrets = ["hunter2".to_owned(), "x-cg-demo".to_owned()];
        assert_eq!(
            redact("key=hunter2 and x-cg-demo again hunter2", &secrets),
            "key=<redacted> and <redacted> again <redacted>"
        );
        assert_eq!(redact("plain text", &secrets), "plain text");
        assert_eq!(redact("anything", &[]), "anything");
        assert_eq!(redact("", &secrets), "");
        let empty = vec![String::new()];
        assert_eq!(redact("value", &empty), "value");
    }

    #[test]
    fn file_log_writes_redacted_lines_to_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.log");
        let log = FileLog::append_at(&path, vec!["top-secret".into()]).unwrap();
        log.info("session started");
        log.trace("error", "payload contained top-secret and more top-secret");
        drop(log);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("session started"), "{content}");
        assert!(content.contains("payload contained <redacted> and more <redacted>"));
        assert!(!content.contains("top-secret"), "secret leaked: {content}");
    }

    #[test]
    fn two_clone_handles_share_one_redacted_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shared.log");
        let first = FileLog::append_at(&path, vec!["secret-value".into()]).unwrap();
        let second = first.clone();
        first.info("first handle writes secret-value");
        second.info("second handle writes clean text");
        drop(first);
        drop(second);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("<redacted>"));
        assert!(!content.contains("secret-value"));
    }
}
