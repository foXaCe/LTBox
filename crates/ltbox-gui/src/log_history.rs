//! Full session transcript on disk, independent of the 500-line GUI tail.
//! Progress is a replaceable row on screen and a five-second sample in exports.

use ltbox_core::live_sink::{Entry, Kind};
use std::{
    cell::RefCell,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    time::{Duration, Instant},
};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

struct Archive {
    file: Option<File>,
    fallback: String,
    error: Option<String>,
}

impl Default for Archive {
    fn default() -> Self {
        match tempfile::tempfile() {
            Ok(file) => Self {
                file: Some(file),
                fallback: String::new(),
                error: None,
            },
            Err(e) => Self {
                file: None,
                fallback: String::new(),
                error: Some(e.to_string()),
            },
        }
    }
}

impl Archive {
    fn append(&mut self, line: &str) {
        if self.error.is_none()
            && let Some(file) = &mut self.file
        {
            match file
                .seek(SeekFrom::End(0))
                .and_then(|_| writeln!(file, "{line}"))
            {
                Ok(_) => return,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        // Never silently discard a line on a full/unavailable temporary disk.
        self.fallback.push_str(line);
        self.fallback.push('\n');
    }

    fn text(&mut self) -> String {
        let mut text = String::new();
        if let Some(file) = &mut self.file
            && let Err(e) = file.rewind().and_then(|_| file.read_to_string(&mut text))
        {
            self.error = Some(e.to_string());
        }
        if let Some(error) = &self.error {
            text.push_str(&format!("[Log] Transcript storage error: {error}\n"));
        }
        text.push_str(&self.fallback);
        text
    }
}

struct Progress {
    key: String,
    line: String,
    last_saved: Option<String>,
    saved_at: Instant,
}

#[derive(Default)]
pub(crate) struct LogHistory {
    archive: RefCell<Archive>,
    progress: Option<Progress>,
}

impl LogHistory {
    pub(crate) fn with_initial(line: &str) -> Self {
        let history = Self::default();
        history.archive.borrow_mut().append(line);
        history
    }

    /// Return an ordinary visible line; progress remains a single separate row.
    pub(crate) fn record(&mut self, entry: Entry, now: Instant) -> Option<String> {
        match entry.kind {
            Kind::Progress { key } => {
                if self.progress.as_ref().is_none_or(|p| p.key != key) {
                    self.finish_progress();
                    self.progress = Some(Progress {
                        key,
                        line: entry.line,
                        last_saved: None,
                        saved_at: now,
                    });
                } else if let Some(progress) = &mut self.progress {
                    progress.line = entry.line;
                    if now.saturating_duration_since(progress.saved_at) >= SAMPLE_INTERVAL {
                        self.archive.borrow_mut().append(&progress.line);
                        progress.last_saved = Some(progress.line.clone());
                        progress.saved_at = now;
                    }
                }
                None
            }
            Kind::Debug => {
                self.archive
                    .borrow_mut()
                    .append(&format!("[Debug] {}", entry.line));
                None
            }
            Kind::Info => {
                self.finish_progress();
                self.archive.borrow_mut().append(&entry.line);
                Some(entry.line)
            }
        }
    }

    pub(crate) fn finish_progress(&mut self) {
        if let Some(progress) = self.progress.take()
            && progress.last_saved.as_deref() != Some(&progress.line)
        {
            self.archive.borrow_mut().append(&progress.line);
        }
    }

    pub(crate) fn progress_line(&self) -> Option<&str> {
        self.progress.as_ref().map(|p| p.line.as_str())
    }

    pub(crate) fn text(&self) -> String {
        let mut text = self.archive.borrow_mut().text();
        if let Some(progress) = &self.progress
            && progress.last_saved.as_deref() != Some(&progress.line)
        {
            text.push_str(&progress.line);
            text.push('\n');
        }
        text.trim_end_matches('\n').to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn progress(key: &str, line: &str) -> Entry {
        Entry {
            line: line.into(),
            kind: Kind::Progress { key: key.into() },
        }
    }

    #[test]
    fn progress_is_replaced_and_sampled_but_last_state_survives_an_error() {
        let mut log = LogHistory::default();
        let now = Instant::now();
        for second in 0..10 {
            assert!(
                log.record(
                    progress("a", &format!("{second}%")),
                    now + Duration::from_secs(second)
                )
                .is_none()
            );
        }
        assert_eq!(log.progress_line(), Some("9%"));
        assert_eq!(log.text(), "5%\n9%");
        log.record(
            Entry::info("write failed: timeout".into()),
            now + Duration::from_secs(10),
        );
        assert_eq!(log.text(), "5%\n9%\nwrite failed: timeout");
        assert!(log.progress_line().is_none());
    }

    #[test]
    fn export_keeps_early_lines_diagnostics_and_repeated_sessions() {
        let mut log = LogHistory::default();
        let now = Instant::now();
        for i in 0..600 {
            log.record(Entry::info(format!("line {i}")), now);
        }
        assert!(log.record(Entry::debug("GPT patch".into()), now).is_none());
        for _ in 0..2 {
            assert!(
                log.record(Entry::info("Sahara connected".into()), now)
                    .is_some()
            );
        }
        let text = log.text();
        assert!(text.starts_with("line 0\n"));
        assert!(text.contains("[Debug] GPT patch"));
        assert_eq!(text.matches("Sahara connected").count(), 2);
    }

    #[test]
    fn save_does_not_duplicate_pending_progress_or_move_append_position() {
        let mut log = LogHistory::with_initial("start");
        let now = Instant::now();
        log.record(progress("a", "10%"), now);
        assert_eq!(log.text(), log.text());
        log.record(progress("b", "20%"), now);
        log.record(Entry::info("done".into()), now);
        assert_eq!(log.text(), "start\n10%\n20%\ndone");
    }

    #[test]
    fn unavailable_temp_storage_preserves_lines_in_memory() {
        let mut archive = Archive {
            file: None,
            fallback: String::new(),
            error: Some("disk unavailable".into()),
        };
        archive.append("first");
        archive.append("error detail");
        let text = archive.text();
        assert!(text.contains("disk unavailable"));
        assert!(text.ends_with("first\nerror detail\n"));
    }
}
