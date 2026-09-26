//! EDL loader picking, validation and per-model memory.

use crate::*;

impl App {
    /// Open the EDL loader picker, filtered to the forms the connected model
    /// accepts. Dedupe across `*SelectLoader` handlers.
    ///
    /// Always shows the dialog, including when the model has a remembered
    /// loader: reaching a loader step that a remembered loader would have
    /// skipped means the user walked Back to it on purpose, and the only
    /// reason to do that is to choose a different file.
    pub(crate) fn pick_loader<F>(&mut self, on_chosen: F) -> Task<Message>
    where
        F: 'static + Send + Fn(Option<String>) -> Message,
    {
        pickers::pick_file_for(self.model_loader_file_spec(), &self.recent_paths, on_chosen)
    }

    /// The remembered EDL loader for the connected model, when it is still
    /// usable. Every wizard consults this to decide whether its loader step
    /// can be filled in and skipped.
    ///
    /// `None` — and therefore the picker — whenever anything is less than
    /// certain: the feature is switched off, no model has been identified yet
    /// (a loader is only ever remembered *against* a model, so with no model
    /// there is nothing to look up), the model has no entry, or the
    /// remembered file has since moved, stopped fitting the model, or lost
    /// the manifest payloads beside it.
    ///
    /// Silent by design. The user never configured this path — it was learned
    /// from an operation they ran — so a stale entry falls back to the picker
    /// rather than accusing them of a misconfiguration.
    pub(crate) fn remembered_loader_for_model(&self) -> Option<String> {
        if !self.remember_edl_loader {
            return None;
        }
        let stored = self
            .remembered_edl_loaders
            .get(&model_memory_key(self.device.model.as_str())?)?;
        let path = std::path::Path::new(stored);
        if !path.is_file() || !self.loader_fits_model(path) {
            return None;
        }
        // Same gate the picker applies, so a remembered manifest that lost its
        // payloads sends the user back to the picker instead of skipping the
        // step onto a loader that cannot load.
        prepare_manifest_pick(path)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// Note which model the operation about to start belongs to, and discard
    /// any unread upload record from the previous one.
    ///
    /// The model is captured *now* because the device is about to re-enumerate
    /// into EDL: `device.model` can be blank, or briefly reset entirely, by the
    /// time the upload succeeds.
    pub(crate) fn arm_loader_memory(&mut self) {
        ltbox_device::edl::clear_uploaded_loader();
        self.loader_memory_model = model_memory_key(self.device.model.as_str());
    }

    /// Record a loader that just completed its Sahara upload against the model
    /// the running operation was armed with. No-op when the feature is off,
    /// when no model was known, or when the entry is already what we'd write.
    pub(crate) fn remember_uploaded_loader(&mut self, loader: &std::path::Path) {
        if !self.remember_edl_loader {
            return;
        }
        let Some(model) = self.loader_memory_model.clone() else {
            return;
        };
        let path = loader.to_string_lossy().into_owned();
        if self.remembered_edl_loaders.get(&model) == Some(&path) {
            return;
        }
        self.remembered_edl_loaders.insert(model, path);
        self.persist_settings();
    }

    /// Drain a completed loader upload published by the EDL layer. Called from
    /// the same tick that drains worker log output.
    pub(crate) fn drain_uploaded_loader(&mut self) {
        if let Some(loader) = ltbox_device::edl::take_uploaded_loader() {
            self.remember_uploaded_loader(&loader);
        }
    }

    /// Apply the remembered loader to whichever advanced-wizard loader-step is
    /// currently open. Pre-fills the wizard's `loader_path` and either advances
    /// directly to the Select step (DumpPhys / FlashPhys — no scan needed) or
    /// fires the GPT scan (FlashParts / DumpParts — Select step requires
    /// populated rows). Called from `AdvConfirm` after a wizard's `_open` flag
    /// flips.
    ///
    /// Returns `Task::none()` when the model has no usable remembered loader —
    /// the caller's existing flow then surfaces the loader step as before, and
    /// the user can still reach it with Back after a skip.
    pub(crate) fn apply_remembered_loader_to_advanced_wizard(&mut self) -> Task<Message> {
        let Some(path) = self.remembered_loader_for_model() else {
            return Task::none();
        };
        if self.advanced_wizard_open.is_flash_parts() {
            // Leave step at 0 (Loader); FlashPartsScanDone advances to
            // Select on success, so jumping past step 0 here would
            // double-advance past Select.
            self.flash_parts.loader_path = Some(path);
            return self.update(Message::FlashParts(FlashPartsMsg::FlashPartsScanStart));
        } else if self.advanced_wizard_open.is_dump_parts() {
            self.dump_parts.loader_path = Some(path);
            return self.update(Message::DumpParts(DumpPartsMsg::DumpPartsScanStart));
        } else if self.advanced_wizard_open.is_dump_phys() {
            // Whole-LUN — no scan. Skip to Select directly.
            self.dump_phys.loader_path = Some(path);
            self.dump_phys.step = 1;
        } else if self.advanced_wizard_open.is_flash_phys() {
            self.flash_phys.loader_path = Some(path);
            self.flash_phys.step = 1;
        }
        Task::none()
    }

    /// Pre-fill the top-level KonaBess wizard from the model's remembered EDL
    /// loader, preserving the former Advanced-tile entry behavior.
    pub(crate) fn apply_remembered_loader_to_konabess(&mut self) {
        let Some(path) = self.remembered_loader_for_model() else {
            return;
        };
        // `remembered_loader_for_model` already fit-checked and gated the path;
        // resolve only to keep the recents bookkeeping identical to a pick.
        if let Ok(loader) = self.resolve_loader_input(&path) {
            self.konabess.loader_path = Some(loader);
            self.konabess.loader_error = None;
            self.konabess.step = 0;
        }
    }

    /// Validate a picked/default EDL loader before device work starts.
    pub(crate) fn validate_loader_path(&mut self, path: &Option<String>) -> Result<String, ()> {
        let Some(p) = path.as_deref() else {
            self.error_msg = Some(self.t("err_loader_not_selected").to_string());
            return Err(());
        };
        let pb = std::path::Path::new(p);
        if !pb.is_file() {
            let msg = tr_args!("err_loader_missing", path = p);
            self.error_msg = Some(msg);
            return Err(());
        }
        // Backstop for loaders that reached a wizard without passing the
        // picker's gate (recents, restored state): decrypt a `.x` manifest and
        // require its images, so a run never gets as far as forcing the device
        // into EDL on a loader that cannot load.
        let prepared = prepare_manifest_pick(pb).map_err(|msg| {
            self.error_msg = Some(msg);
        })?;
        Ok(prepared.to_string_lossy().into_owned())
    }

    /// Route a picked loader into one wizard's `(loader_path, loader_error)`
    /// pair. Cancelling changes nothing; a resolved pick clears the error; a
    /// rejected pick clears the path, so a wizard can never advance on the
    /// loader it just refused.
    pub(crate) fn apply_loader_pick<F>(&mut self, picked: Option<String>, set: F)
    where
        F: FnOnce(&mut Self, Option<String>, Option<String>),
    {
        let Some(p) = picked else { return };
        match self.resolve_loader_input(&p) {
            Ok(loader) => set(self, Some(loader), None),
            Err(msg) => set(self, None, Some(msg)),
        }
    }

    /// Resolve loader input from the unified picker path. The picker offers
    /// `.melf` or a Sahara `.xml`/`.x` manifest according to the connected
    /// model; a directory is also accepted, for older recents entries, and
    /// resolved via [`find_edl_loader`].
    pub(crate) fn resolve_loader_input(
        &mut self,
        selected_path: &str,
    ) -> std::result::Result<String, String> {
        let path = std::path::Path::new(selected_path);
        if path.is_file() {
            self.remember_recent(pickers::PickerKind::File, selected_path);
            // TB323FU model gate: if the user picked a `.melf` but the
            // device is a TB323FU, upgrade to the
            // `qsahara_device_programmer.xml` manifest sitting in the
            // same folder. If the manifest is missing the .melf
            // alone is wrong and would fail mid-Sahara — abort up
            // front. Performed during resolve so the wizard's Confirm
            // step shows the correct path.
            if self.requires_sahara_manifest()
                && is_melf_loader(path)
                && let Some(parent) = path.parent()
            {
                if let Some(manifest) = resolve_sahara_manifest(parent) {
                    return prepare_manifest_pick(&manifest)
                        .map(|p| p.to_string_lossy().to_string());
                }
                return Err(tr_args!(
                    "err_efisp_loader_manifest_required",
                    model = self.device.model.as_str(),
                    path = path.display()
                ));
            }
            // Multi-image manifest picked directly, in either form. The
            // encrypted `.x` is decrypted to its sibling `.xml` here rather
            // than in `EdlSession::open`, because the completeness check
            // below needs the `<image>` list while the user is still in the
            // picker — a manifest missing its payloads must not reach a
            // wizard's Next button.
            if ltbox_core::sahara_xml::is_encrypted_manifest_filename(path)
                || ltbox_core::sahara_xml::is_manifest_filename(path)
            {
                return prepare_manifest_pick(path).map(|p| p.to_string_lossy().to_string());
            }
            if is_loader_file(path) {
                return Ok(selected_path.to_string());
            }
            return Err(tr_args!(
                "err_unsupported_loader_file",
                path = selected_path
            ));
        }

        if path.is_dir() {
            self.remember_recent(pickers::PickerKind::LoaderFolder, selected_path);
            return find_edl_loader(path)
                .map(|p| p.to_string_lossy().to_string())
                .ok_or_else(|| tr_args!("err_loader_not_found_in_path", path = selected_path));
        }

        Err(tr_args!("err_path_missing", path = selected_path))
    }

    /// Whether the polled device is a TB323FU. Drives the multi-image
    /// EDL loader path: TB323FU doesn't accept a single
    /// `xbl_s_devprg_ns.melf`; it needs the full
    /// `qsahara_device_programmer.xml` manifest + the per-id ELF /
    /// MBN payloads it references. The loader resolver upgrades a
    /// stray `.melf` selection to the manifest when one exists in
    /// the same folder; if not, it aborts up front rather than
    /// failing mid-Sahara.
    pub(crate) fn requires_sahara_manifest(&self) -> bool {
        self.model_capabilities().requires_sahara_manifest
    }

    /// True when `path`'s extension is the EDL loader form the connected device
    /// needs: manifest-route models load `.xml` / `.x`; other connected models
    /// load `.melf`. With no connected device, either loader form is valid.
    /// Inspects only the picked file's own extension, never the `.mbn` / `.elf`
    /// images a manifest references internally.
    pub(crate) fn loader_fits_model(&self, path: &std::path::Path) -> bool {
        if self.device.connection == ConnectionStatus::None {
            return loader_ext_fits_model(false, path) || loader_ext_fits_model(true, path);
        }
        loader_ext_fits_model(self.model_capabilities().requires_sahara_manifest, path)
    }

    pub(crate) fn loader_picker_exts(&self) -> &'static [&'static str] {
        loader::loader_picker_extensions(
            self.device.connection != ConnectionStatus::None && !self.device.model.is_empty(),
            self.device.connection != ConnectionStatus::None && self.requires_sahara_manifest(),
        )
    }

    pub(crate) fn model_loader_file_spec(&self) -> pickers::FilePickSpec {
        let exts = self.loader_picker_exts();
        pickers::FilePickSpec::single()
            .with_filter(format!("EDL loader ({})", exts.join(" / ")), exts)
    }
}
