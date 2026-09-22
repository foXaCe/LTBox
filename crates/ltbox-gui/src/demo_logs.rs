//! Device-free copy review using production translations and interpolation.
//! This catalogue is synthetic; it does not claim to execute device workflows.
use crate::log_messages::LiveLabels;
use crate::operation_phase::OperationPhaseKind;
use crate::translations::{LANGUAGES, Language, Translations};
use ltbox_core::i18n::format_template;
use std::path::Path;

fn sample(name: &str) -> String {
    match name {
        "path" | "output" | "source" | "loader" => "C:/LTBox/backup/init_boot.img",
        "image" | "name" => "init_boot.img",
        "part" | "partition" | "label" | "partitions" => "init_boot_a",
        "images" => "init_boot.img + vbmeta.img",
        "error" | "reason" => "Access denied (sample)",
        "package" => "com.lenovo.ota",
        "size" => "64 MB",
        "bytes" => "64000000",
        "lun" | "index" | "from" => "0",
        "count" | "total" | "to" => "3",
        "phase" | "success" => "2",
        "port" => "COM3",
        "version" | "tag" => "v1.2.3",
        "elapsed" => "1.2 s",
        _ => return format!("<{name}>"),
    }
    .into()
}

fn render_sample(template: &str) -> String {
    let values: Vec<_> = template
        .split('{')
        .skip(1)
        .filter_map(|tail| tail.split_once('}'))
        .map(|(name, _)| (name, sample(name)))
        .collect();
    format_template(template, &values)
}

fn phase(table: &Translations, kind: OperationPhaseKind, index: usize) -> String {
    format_template(
        table.t("live_phase_marker"),
        &[
            ("phase", (index + 1).to_string()),
            ("total", kind.keys().len().to_string()),
            ("label", table.t(kind.keys()[index]).into()),
        ],
    )
}

pub(crate) fn render(language: Language) -> String {
    let table = Translations::load(language);
    let labels = LiveLabels::new(|key| table.t(key).to_owned());
    let mut output = String::from(
        "SYNTHETIC LOG COPY CATALOGUE — no device operations executed\n\
         Values in <brackets> are sample placeholders.\n\n\
         COMMON + INDIVIDUAL MESSAGE COMPOSITION\n",
    );
    let path = Path::new("C:/LTBox/backup");
    for line in [
        format!("[Root] {}", phase(&table, OperationPhaseKind::Root, 2)),
        format!("[Root] {}", labels.root_resolved("init_boot_a", 4)),
        format!("[Root] {}", phase(&table, OperationPhaseKind::Root, 3)),
        format!(
            "[Root] {}",
            labels.root_backup_copy("init_boot.img + vbmeta.img", path)
        ),
        format!("[EDL] {}", labels.closing_dump),
        format!("[Root] {}", phase(&table, OperationPhaseKind::Root, 4)),
    ] {
        output.push_str(&line);
        output.push('\n');
    }
    output.push_str("\nPOST-WRITE BACKUP LOCATION\n");
    output.push_str(&format!("[Root] {}\n", labels.backup_saved(path)));
    output.push_str("\nPARTIAL PACKAGE RESULT\n");
    for key in ["live_adb_uninstall_failed", "live_sysupdate_disabled"] {
        let tag = if key == "live_sysupdate_disabled" {
            "SysUpdate"
        } else {
            "ADB"
        };
        output.push_str(&format!("[{tag}] {}\n", render_sample(table.t(key))));
    }
    output.push_str("\nDECOMPRESSION PROGRESS THEN COMPLETION\n");
    for key in ["live_flash_zst_progress", "live_flash_zst_done"] {
        output.push_str(&format!("[Flash] {}\n", render_sample(table.t(key))));
    }
    output.push_str("\nCOMMON PHASE SEQUENCES (not execution transcripts)\n");
    for kind in OperationPhaseKind::all() {
        output.push_str(&format!("\n{kind:?}\n"));
        for index in 0..kind.keys().len() {
            output.push_str(&phase(&table, *kind, index));
            output.push('\n');
        }
    }
    output.push_str("\nINDIVIDUAL TEMPLATES (callsite prefixes are in log_catalog.py output)\n");
    let mut keys: Vec<_> = table
        .primary
        .keys()
        .filter(|key| {
            key.starts_with("live_")
                || key.starts_with("log_")
                || key.starts_with("country_reason_")
        })
        .collect();
    keys.sort();
    for key in keys {
        output.push_str(&format!("{key}\n{}\n\n", render_sample(table.t(key))));
    }
    output
}

pub(crate) fn export(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    for &language in LANGUAGES {
        std::fs::write(
            directory.join(format!("{}.log", language.code())),
            render(language),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_renders_every_language_without_template_tokens() {
        for &language in LANGUAGES {
            let rendered = render(language);
            assert!(
                !rendered.contains('{'),
                "{}: unresolved token",
                language.code()
            );
            assert!(rendered.contains("[Root] "));
            assert!(rendered.contains("C:/LTBox/backup/init_boot.img"));
            assert!(rendered.contains("DumpPhysical"));
        }
    }
}
