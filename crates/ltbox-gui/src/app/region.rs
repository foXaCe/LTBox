//! Firmware folder region inference and the Lenovo region/QFIL/OTA lookups.

use crate::*;

impl App {
    /// Record the picked flash firmware folder and flag whether it ships an EDL
    /// loader (mirroring the worker's dir-then-parent lookup). When it does not,
    /// pre-fill the model's remembered loader when there is one; the
    /// folder step otherwise requires the user to pick one before advancing.
    pub(crate) fn set_flash_firmware_folder(&mut self, path: String) {
        let was_after_folder = matches!(
            self.flash.current_step(),
            FlashStep::Bootloader | FlashStep::Confirm
        );
        self.flash.reset_firmware_identity();
        // Users frequently pick the extracted firmware ROOT instead of the
        // `image` folder LTBox flashes; retarget to a direct `image/` child
        // when one exists so the common mis-selection just works.
        let path = redirect_str(path);
        let dir = std::path::Path::new(&path);
        // The folder's own loader counts only when it is actually usable: a
        // Sahara manifest is decrypted (if `.x`) and checked for its images
        // right here, so a half-extracted pack demands an external loader on
        // the folder step instead of failing once the device is in EDL.
        let folder_loader = find_firmware_loader(dir);
        let folder_loader_error = folder_loader
            .as_deref()
            .and_then(|loader| prepare_manifest_pick(loader).err());
        self.flash.loader_required = folder_loader.is_none() || folder_loader_error.is_some();
        self.flash.loader_override = if self.flash.loader_required {
            self.remembered_loader_for_model()
        } else {
            None
        };
        self.flash.loader_error = folder_loader_error;
        self.flash.firmware_folder = Some(path.clone());
        self.flash.set_firmware_rollback_indices(&path);
        if was_after_folder || (self.flash.loader_required && self.flash.loader_override.is_none())
        {
            self.flash.set_step(FlashStep::Folder);
        }
    }

    /// Map cached PTSTPD `SaleArea` for the connected device → `DeviceRegion`.
    /// `"CN"` → PRC, JSON null → ROW, anything else → `None`. Cache-only.
    pub(crate) fn inferred_flash_region(&self) -> Option<DeviceRegion> {
        if self.device.serial.is_empty() {
            return None;
        }
        let info = self.queries.info_cache.get(&self.device.serial)?;
        region_from_salearea(info)
    }

    /// Prepare the Flash region step after `flash.reset()` without initiating
    /// a lookup. Demo scenes keep their deterministic state, while PRC-only
    /// models still skip the single-choice step.
    pub(crate) fn prepare_flash_region_on_entry(&mut self) -> Task<Message> {
        #[cfg(feature = "demo")]
        if demo::prepare_flash_region_on_entry(self) {
            return Task::none();
        }
        if self.is_prc_only() {
            self.flash.region_selection = Some(FlashRegionSelection::Manual(DeviceRegion::Prc));
            self.flash.device_region = Some(DeviceRegion::Prc);
            self.flash.step = 1;
        }
        Task::none()
    }

    /// Kick off region auto-detection after the automatic-selection option is
    /// chosen and Next is pressed:
    /// * PRC-only model → preselect PRC and jump to the target step.
    /// * usable polled serial → probe PTSTPD (region step shows an indicator).
    /// * no usable serial → open the manual-serial prompt.
    ///
    /// On a fetch failure / inconclusive SaleArea the handler falls back to the
    /// manual PRC/ROW cards, so this never blocks the wizard.
    pub(crate) fn begin_flash_region_auto(&mut self) -> Task<Message> {
        if self.is_prc_only() {
            // PRC-only SKU: no lookup needed; skip straight to the target step.
            self.flash.device_region = Some(DeviceRegion::Prc);
            self.flash.step = 1;
            return Task::none();
        }
        if self.has_pollable_serial() {
            let serial = self.device.serial.trim().to_string();
            return self.start_region_probe(serial);
        }
        // Serial comes only from an ADB/fastboot poll; ask for it manually.
        self.flash_serial_prompt = Some(String::new());
        Task::none()
    }

    /// Mint a fresh probe id, mark it pending (shows the progress indicator), and spawn
    /// the lookup. The id is the staleness token the result handler checks.
    pub(crate) fn start_region_probe(&mut self, serial: String) -> Task<Message> {
        let id = self.queries.start_region();
        self.spawn_auto_region_fetch(id, serial)
    }

    /// Whether the polled serial is usable for an automatic region lookup: the
    /// device is in an ADB/fastboot state (the only ones that yield a serial)
    /// and the serial has the expected `HA…` prefix (guards against a garbled
    /// read). When false, region detection falls back to the manual prompt.
    pub(crate) fn has_pollable_serial(&self) -> bool {
        matches!(
            self.device.connection,
            ConnectionStatus::Adb | ConnectionStatus::AdbRecovery | ConnectionStatus::Fastboot
        ) && self.device.serial.trim().starts_with("HA")
    }

    /// Off-thread PTSTPD fetch for auto region detection. Reuses the device
    /// -info cache when the serial is already known this session; otherwise
    /// fetches. Result arrives as `FlashMsg::FlashAutoRegionFetched`.
    pub(crate) fn spawn_auto_region_fetch(&self, id: u64, serial: String) -> Task<Message> {
        if let Some(info) = self.queries.info_cache.get(&serial).cloned() {
            return Task::done(Message::Flash(FlashMsg::FlashAutoRegionFetched(
                id,
                serial,
                Ok(info),
            )));
        }
        // The fallback (heavy-thread spawn failure / panic) carries the same id
        // + serial so the handler clears the progress indicator and falls back to manual
        // instead of treating it as stale.
        let serial_fb = serial.clone();
        task_heavy(
            move || {
                let r =
                    ltbox_core::lenovo_info::fetch_machine_info(&serial).map_err(|e| e.to_string());
                (id, serial, r)
            },
            |(id, s, r)| Message::Flash(FlashMsg::FlashAutoRegionFetched(id, s, r)),
            move |e| (id, serial_fb, Err(e)),
        )
    }

    /// Re-populate `ota_changelog_editor` from the current popup
    /// state. Picks `desc_cn` for the Chinese GUI locale (with
    /// `desc_en` fallback when `desc_cn` is empty), `desc_en`
    /// otherwise. Called from both `OtaOpen` (cache restore) and
    /// `OtaFetched` (fresh fetch) so the editor's contents stay in
    /// lockstep with whatever the popup is about to render.
    pub(crate) fn seed_ota_changelog_editor(&mut self, state: &OtaPopupState) {
        let editor_text = if let OtaPopupState::Ready(u) = state {
            let prefer_cn = matches!(self.settings.language, Language::Zh);
            let raw = if prefer_cn && !u.desc_cn.trim().is_empty() {
                &u.desc_cn
            } else if !u.desc_en.trim().is_empty() {
                &u.desc_en
            } else {
                &u.desc_cn
            };
            ltbox_core::lenovo_ota::format_changelog(raw)
        } else {
            String::new()
        };
        self.ota_changelog_editor = iced::widget::text_editor::Content::with_text(&editor_text);
    }

    /// Build the off-thread QFIL-fetch task for `serial`. Reuses the
    /// device-info cache for MTM + SaleArea when the device-info popup already
    /// fetched them this session; otherwise the worker fetches machine info
    /// itself. Result arrives as `Message::QfilFetched`.
    pub(crate) fn spawn_qfil_fetch(&mut self, serial: String) -> Task<Message> {
        use ltbox_core::lenovo_info::FieldValue;
        let cached: Option<(String, String)> = self.queries.info_cache.get(&serial).map(|info| {
            let field = |k: &str| match info.field(k) {
                FieldValue::Value(s) => s,
                _ => String::new(),
            };
            (field("MTM"), field("SaleArea"))
        });
        let serial_for_task = serial;
        let task = task_heavy(
            move || {
                let outcome = resolve_qfil(&serial_for_task, cached);
                (serial_for_task, outcome)
            },
            |(s, r)| Message::QfilFetched(s, r),
            |e| (String::new(), Err(e)),
        );
        self.queries.track_lookup(LookupKind::Qfil, task)
    }
}
