//! ARB (anti-rollback) detection worker and its UTC timestamp helpers.

use crate::*;

/// Format a unix timestamp (seconds) as `YYYY-MM-DD HH:MM:SS UTC`.
/// Pure stdlib, independent of the local-time backup naming. Uses Howard Hinnant's civil-from-days
/// algorithm so the proleptic Gregorian conversion stays correct
/// across leap years and century boundaries without a calendar table.
pub(crate) fn format_unix_timestamp_utc(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let rem = (ts % 86_400) as u32;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02} UTC")
}

/// Date-only rendering of a rollback index (`YYYY-MM-DD`, UTC). The
/// rollback-index popup cycles through this as its most human form —
/// the time-of-day component carries no meaning for a rollback floor.
pub(crate) fn format_unix_date_utc(ts: u64) -> String {
    let (y, mo, d) = civil_from_days((ts / 86_400) as i64);
    format!("{y:04}-{mo:02}-{d:02}")
}

/// Howard Hinnant `civil_from_days`: (days since 1970-01-01) →
/// `(year, month, day)` in the proleptic Gregorian calendar.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Whether the rollback floors can be read over fastboot instead of needing
/// an EDL programmer. The model side is a capability, not a model list — see
/// `ModelCapabilities::rollback_floor_via_fastboot`.
pub(crate) fn rollback_query_uses_fastboot(conn: ConnectionStatus, model: &str) -> bool {
    matches!(
        conn,
        ConnectionStatus::Adb | ConnectionStatus::AdbRecovery | ConnectionStatus::Fastboot
    ) && ltbox_core::model::capabilities(model).rollback_floor_via_fastboot
}

impl App {
    pub(crate) fn rollback_query_needs_loader(&self) -> bool {
        !rollback_query_uses_fastboot(self.device.connection, &self.device.model)
    }

    pub(crate) fn can_query_rollback(&self) -> bool {
        crate::model::device::is_rollback_protected_model(&self.device.model)
            && self.device_reachable()
            && (!self.rollback_query_needs_loader()
                || self.adv_wizard.file_path.as_deref().is_some_and(|path| {
                    std::path::Path::new(path).is_file()
                        && self.loader_fits_model(std::path::Path::new(path))
                }))
    }
}

/// Report stored fastboot floors on supported models, otherwise inspect AVB
/// metadata over EDL. An EDL image index is not a hardware rollback floor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn detect_arb_run(
    conn: ConnectionStatus,
    device_model: String,
    loader_path: Option<String>,
    _i_anti: &str,
    i_not: &str,
    i_reboot_fastboot: &str,
    i_reboot_system: &str,
    i_edl_dump: &str,
    phases: PhaseReporter,
    log: &mut Vec<String>,
) -> Result<(), String> {
    use ltbox_device::adb::AdbManager;
    use ltbox_device::fastboot::FastbootDevice;
    if !crate::model::device::is_rollback_protected_model(&device_model) {
        return Err(i_not.to_string());
    }
    ltbox_core::live!(log, "[ARB] {}", phases.marker(1));
    if rollback_query_uses_fastboot(conn, &device_model) {
        if conn != ConnectionStatus::Fastboot {
            ltbox_core::live!(log, "[ARB] {i_reboot_fastboot}");
            AdbManager::new()
                .shell("reboot bootloader")
                .map_err(|e| e.to_string())?;
            FastbootDevice::wait_for_device().map_err(|e| e.to_string())?;
        }
        ltbox_core::live!(log, "[ARB] {}", phases.marker(2));
        let mut dev = FastbootDevice::open().map_err(|e| e.to_string())?;
        let vars = dev.get_all_vars().map_err(|e| e.to_string())?;
        let mut indices: Vec<_> = vars.rollback_indices.into_iter().collect();
        indices.sort_by_key(|(index, _)| *index);
        ltbox_core::live!(log, "[ARB] {}", phases.marker(4));
        for (index, value) in &indices {
            ltbox_core::live!(log, "[ARB] stored_rollback_index:{index} = {value}");
        }
        ltbox_core::live!(log, "[ARB] {}", phases.marker(5));
        ltbox_core::live!(log, "[ARB] {i_reboot_system}");
        dev.reboot().map_err(|e| e.to_string())?;
        if indices.is_empty() {
            return Err("fastboot did not report rollback indices".into());
        }
        return Ok(());
    }

    let loader = loader_path
        .filter(|p| std::path::Path::new(p).is_file())
        .ok_or_else(|| "An EDL loader is required for rollback inspection".to_string())?;
    // Capture the slot before leaving a readable transport. When starting in
    // EDL no active-slot claim can be made, so report both slots explicitly.
    let slots = if matches!(
        conn,
        ConnectionStatus::Adb | ConnectionStatus::AdbRecovery | ConnectionStatus::Fastboot
    ) {
        vec![
            ltbox_device::controller::poll_active_slot(std::time::Duration::from_secs(30), log)
                .map_err(|e| e.to_string())?,
        ]
    } else {
        vec!["_a".to_string(), "_b".to_string()]
    };
    ltbox_core::live!(log, "[ARB] {}", phases.marker(3));
    ensure_edl(conn, "ARB", log).map_err(|()| "Failed to enter EDL".to_string())?;
    let temporary = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut session = open_edl_session(std::path::Path::new(&loader), log)?;
    ltbox_core::live!(log, "[ARB] {i_edl_dump}");
    let both_slots = slots.len() > 1;
    if let Err(error) = dump_slot_rollback_indices(&mut session, &slots, temporary.path(), log) {
        // Dropping the session returns an EDL start to EDL for a retry.
        // Otherwise nothing was written, so boot back to the system the
        // query started from.
        if !both_slots {
            session.reset_tolerant(log);
        }
        return Err(error);
    }
    ltbox_core::live!(log, "[ARB] {}", phases.marker(4));
    ltbox_core::live!(
        log,
        "[ARB] {}",
        ltbox_core::i18n::tr("arb_edl_index_trust_warning")
    );
    ltbox_core::live!(log, "[ARB] {}", phases.marker(5));
    ltbox_core::live!(log, "[ARB] {i_reboot_system}");
    session.reset_tolerant(log);
    Ok(())
}

/// Dump `boot` + `vbmeta_system` per slot and log each image's rollback index.
///
/// With both slots (an EDL start, where the active slot is unknown), a slot
/// whose images carry no parseable AVB metadata is reported and skipped:
/// stock rawprograms leave the `_b` images unwritten, so a factory or freshly
/// flashed device has nothing to parse there. A failed dump is a transport
/// error and still aborts, and at least one slot must be readable.
fn dump_slot_rollback_indices(
    session: &mut ltbox_device::edl::EdlSession,
    slots: &[String],
    temporary: &std::path::Path,
    log: &mut Vec<String>,
) -> Result<(), String> {
    let tolerate_unreadable_slot = slots.len() > 1;
    let mut readable_slots = 0usize;
    for slot in slots {
        let mut slot_readable = true;
        for (name, lun) in [("boot", 4), ("vbmeta_system", 0)] {
            let partition = format!("{name}{slot}");
            let output = temporary.join(format!("{partition}.img"));
            session
                .dump_partition(&partition, &output, 0, lun, log)
                .map_err(|e| format!("dump {partition}: {e}"))?;
            match ltbox_patch::avb::extract_image_avb_info(&output) {
                Ok(info) => {
                    ltbox_core::live!(log, "[ARB] {partition} = {}", info.rollback_index);
                }
                Err(e) if tolerate_unreadable_slot => {
                    ltbox_core::live!(log, "[ARB] {partition}: {e}");
                    slot_readable = false;
                }
                Err(e) => return Err(format!("{partition} AVB: {e}")),
            }
        }
        if slot_readable {
            readable_slots += 1;
        }
    }
    if readable_slots == 0 {
        return Err("no slot has readable AVB metadata".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod query_tests {
    use super::*;
    #[test]
    fn only_known_fastboot_models_use_stored_floors() {
        for model in ["TB321FU", "TB520FU"] {
            assert!(rollback_query_uses_fastboot(ConnectionStatus::Adb, model));
            assert!(rollback_query_uses_fastboot(
                ConnectionStatus::Fastboot,
                model
            ));
            assert!(!rollback_query_uses_fastboot(ConnectionStatus::Edl, model));
        }
        for model in ["", "unknown", "TB322FC", "TB320FC", "TB324ZC"] {
            assert!(!rollback_query_uses_fastboot(ConnectionStatus::Adb, model));
        }
    }

    #[test]
    fn exempt_device_is_gated_and_unknown_edl_requires_a_loader() {
        let mut app = App::default();
        app.device.connection = ConnectionStatus::Adb;
        app.device.model = "TB322FC".into();
        assert!(!app.can_query_rollback());
        assert_eq!(
            app.update(Message::Adv(AdvMsg::AdvDetectArbExecStart))
                .units(),
            0
        );
        assert!(!app.operation.is_running());
        app.device.model.clear();
        app.device.connection = ConnectionStatus::Edl;
        assert!(app.rollback_query_needs_loader());
        assert!(!app.can_query_rollback());
        let loader = tempfile::Builder::new().suffix(".melf").tempfile().unwrap();
        app.adv_wizard.file_path = Some(loader.path().to_string_lossy().into_owned());
        assert!(app.can_query_rollback());
    }
}
