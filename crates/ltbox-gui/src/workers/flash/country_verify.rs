//! Country write/readback orchestration shared by both country-change callers.

use crate::CountryPatchProgress;
use ltbox_core::{live, tr_args};
use std::path::Path;

// Keep the seam at the device boundary so tests exercise the production
// ordering, comparison, and aggregate outcome without a connected device.
pub(super) trait CountrySession {
    fn flash_partition(
        &mut self,
        label: &str,
        path: &Path,
        slot: u8,
        lun: u8,
        log: &mut Vec<String>,
    ) -> Result<(), String>;
    fn dump_partition(
        &mut self,
        label: &str,
        path: &Path,
        slot: u8,
        lun: u8,
        log: &mut Vec<String>,
    ) -> Result<(), String>;
}

impl CountrySession for ltbox_device::edl::EdlSession {
    fn flash_partition(
        &mut self,
        label: &str,
        path: &Path,
        slot: u8,
        lun: u8,
        log: &mut Vec<String>,
    ) -> Result<(), String> {
        Self::flash_partition(self, label, path, slot, lun, log).map_err(|e| e.to_string())
    }
    fn dump_partition(
        &mut self,
        label: &str,
        path: &Path,
        slot: u8,
        lun: u8,
        log: &mut Vec<String>,
    ) -> Result<(), String> {
        Self::dump_partition(self, label, path, slot, lun, log).map_err(|e| e.to_string())
    }
}

pub(super) fn flash_and_verify_country(
    session: &mut impl CountrySession,
    label: &str,
    lun: u8,
    patched_path: &Path,
    verify_path: &Path,
    country_progress: &mut CountryPatchProgress,
    log: &mut Vec<String>,
) {
    if let Err(e) = session.flash_partition(label, patched_path, 0, lun, log) {
        ltbox_core::live!(
            log,
            "[Country] {}",
            tr_args!(
                "live_country_flash_failed",
                label = label,
                error = e.to_string()
            )
        );
        country_progress.mark_failed(label, tr_args!("country_reason_flash_failed", error = e));
    } else {
        live!(
            log,
            "[Country] {}",
            tr_args!("live_country_patched_flashed", label = label)
        );
        // Read the partition back: a write the programmer acknowledged
        // is not yet proof the device holds the patched bytes.
        let verified = session
            .dump_partition(label, verify_path, 0, lun, log)
            .map_err(|e| tr_args!("country_reason_verify_failed", error = e))
            .and_then(|_| {
                super::files_identical(patched_path, verify_path)
                    .map_err(|e| tr_args!("country_reason_verify_failed", error = e))
            });
        match verified {
            Ok(true) => {
                live!(
                    log,
                    "[Country] {}",
                    tr_args!("live_country_verified", label = label)
                );
                country_progress.mark_flashed(label);
            }
            Ok(false) => {
                let reason = ltbox_core::i18n::tr("country_reason_verify_mismatch");
                ltbox_core::live!(
                    log,
                    "[Country] {}",
                    tr_args!(
                        "live_country_partition_status",
                        label = label,
                        reason = reason
                    )
                );
                country_progress.mark_failed(label, reason);
            }
            Err(reason) => {
                ltbox_core::live!(
                    log,
                    "[Country] {}",
                    tr_args!(
                        "live_country_partition_status",
                        label = label,
                        reason = reason
                    )
                );
                country_progress.mark_failed(label, reason);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[derive(Clone, Copy, Debug)]
    enum Scenario {
        Match,
        Mismatch,
        DumpError,
        FlashError,
        MissingReadback,
    }

    #[derive(Debug, PartialEq)]
    struct Call {
        operation: &'static str,
        label: String,
        path: PathBuf,
        slot: u8,
        lun: u8,
    }

    struct ScriptedSession {
        scenario: Scenario,
        calls: Vec<Call>,
    }

    impl CountrySession for ScriptedSession {
        fn flash_partition(
            &mut self,
            label: &str,
            path: &Path,
            slot: u8,
            lun: u8,
            _: &mut Vec<String>,
        ) -> Result<(), String> {
            self.calls.push(Call {
                operation: "flash",
                label: label.into(),
                path: path.into(),
                slot,
                lun,
            });
            assert_eq!(std::fs::read(path).unwrap(), b"patched country");
            if matches!(self.scenario, Scenario::FlashError) {
                Err("write rejected".into())
            } else {
                Ok(())
            }
        }

        fn dump_partition(
            &mut self,
            label: &str,
            path: &Path,
            slot: u8,
            lun: u8,
            _: &mut Vec<String>,
        ) -> Result<(), String> {
            self.calls.push(Call {
                operation: "dump",
                label: label.into(),
                path: path.into(),
                slot,
                lun,
            });
            match self.scenario {
                Scenario::Match => std::fs::write(path, b"patched country").unwrap(),
                Scenario::Mismatch => std::fs::write(path, b"old country    ").unwrap(),
                Scenario::DumpError => return Err("read rejected".into()),
                Scenario::MissingReadback => {}
                Scenario::FlashError => panic!("must not read back after a rejected write"),
            }
            Ok(())
        }
    }

    #[test]
    fn country_write_requires_successful_matching_readback() {
        for scenario in [
            Scenario::Match,
            Scenario::Mismatch,
            Scenario::DumpError,
            Scenario::FlashError,
            Scenario::MissingReadback,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let patched = dir.path().join("proinfo.patched.img");
            let readback = dir.path().join("proinfo.verify.img");
            std::fs::write(&patched, b"patched country").unwrap();
            let mut session = ScriptedSession {
                scenario,
                calls: Vec::new(),
            };
            let mut progress = CountryPatchProgress::new(&["proinfo"]);
            let mut log = Vec::new();
            flash_and_verify_country(
                &mut session,
                "proinfo",
                4,
                &patched,
                &readback,
                &mut progress,
                &mut log,
            );

            let mut expected = vec![Call {
                operation: "flash",
                label: "proinfo".into(),
                path: patched,
                slot: 0,
                lun: 4,
            }];
            if !matches!(scenario, Scenario::FlashError) {
                expected.push(Call {
                    operation: "dump",
                    label: "proinfo".into(),
                    path: readback,
                    slot: 0,
                    lun: 4,
                });
            }
            assert_eq!(session.calls, expected, "{scenario:?}");
            assert_eq!(
                progress.finish().is_ok(),
                matches!(scenario, Scenario::Match),
                "{scenario:?}"
            );
            let verified = tr_args!("live_country_verified", label = "proinfo");
            assert_eq!(
                log.iter()
                    .any(|line| line == &format!("[Country] {verified}")),
                matches!(scenario, Scenario::Match),
                "{scenario:?}"
            );
        }
    }
}
