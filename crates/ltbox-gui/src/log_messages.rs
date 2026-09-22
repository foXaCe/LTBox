//! Complete translated log messages shared by workers and the log demo.
use ltbox_core::i18n::format_template;
use std::path::Path;

#[derive(Debug, Clone)]
pub(crate) struct LiveLabels {
    pub(crate) closing_dump: String,
    pub(crate) flash_completed: String,
    pub(crate) adb_no_kver: String,
    backup_saved: String,
    root_resolved: String,
    root_backup_copy: String,
}

impl LiveLabels {
    pub(crate) fn new(t: impl Fn(&str) -> String) -> Self {
        Self {
            closing_dump: t("live_closing_dump_session"),
            flash_completed: t("live_flash_completed"),
            adb_no_kver: t("live_adb_no_kver"),
            backup_saved: t("live_backup_saved"),
            root_resolved: t("live_root_resolved"),
            root_backup_copy: t("live_root_backup_copy"),
        }
    }
    pub(crate) fn backup_saved(&self, path: &Path) -> String {
        format_template(&self.backup_saved, &[("path", path.display().to_string())])
    }
    pub(crate) fn root_resolved(&self, partitions: &str, lun: u8) -> String {
        format_template(
            &self.root_resolved,
            &[("partitions", partitions.into()), ("lun", lun.to_string())],
        )
    }
    pub(crate) fn root_backup_copy(&self, images: &str, path: &Path) -> String {
        format_template(
            &self.root_backup_copy,
            &[
                ("images", images.into()),
                ("path", path.display().to_string()),
            ],
        )
    }
}
