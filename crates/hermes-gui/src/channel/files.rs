//! Files: uploading, pasting pictures and downloading.

use super::*;

impl Shell {
    /// Adds an upload chip and starts uploading `path`.
    pub fn start_upload(
        &mut self,
        key: &ChannelKey,
        path: std::path::PathBuf,
        name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.next_upload += 1;
        let local = self.next_upload;
        let name = name.unwrap_or_else(|| {
            path.file_name()
                .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned())
        });
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // Too large for the server: said at once, nothing is read or sent.
        let refusal = self
            .core
            .store()
            .instance(&key.0)
            .and_then(|s| s.upload_refusal(size));
        let Some(view) = self.chats.get_mut(key) else {
            return;
        };
        let refused = refusal.is_some();
        view.uploads.push(Upload {
            local,
            name,
            size,
            state: refusal.map_or(UploadState::Uploading, UploadState::Failed),
        });
        if refused {
            cx.notify();
            return;
        }
        let core = self.core.clone();
        let inst = key.0.clone();
        let channel = key.1;
        let key = key.clone();
        self.run(
            cx,
            async move {
                // A picture's size, a sound's waveform and a video's length go with it.
                let file = path.clone();
                let info = tokio::task::spawn_blocking(move || {
                    let measured = crate::media::measure_file(&file);
                    hermes_client_core::secure::FileInfo {
                        size: image::image_dimensions(&file).ok().or(measured.size),
                        poster: measured.poster,
                        waveform: measured.waveform,
                        duration_ms: measured.duration_ms,
                    }
                })
                .await
                .unwrap_or_default();
                core.upload_to(&inst, channel, &path, info)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |this, result, _, _| {
                if let Some(upload) = this
                    .chats
                    .get_mut(&key)
                    .and_then(|v| v.uploads.iter_mut().find(|u| u.local == local))
                {
                    upload.state = match result {
                        Ok(attachment) => UploadState::Ready(attachment),
                        Err(err) => UploadState::Failed(err),
                    };
                }
            },
        );
        cx.notify();
    }

    pub(super) fn pick_files(&mut self, key: ChannelKey, cx: &mut Context<Self>) {
        self.run(
            cx,
            async move {
                rfd::AsyncFileDialog::new()
                    .set_title("Attach files")
                    .pick_files()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| f.path().to_path_buf())
                    .collect::<Vec<_>>()
            },
            move |this, paths, _, cx| {
                for path in paths.into_iter().filter(|p| p.is_file()) {
                    this.start_upload(&key, path, None, cx);
                }
                this.focus_composer = true;
            },
        );
    }

    /// Ctrl+V: an image on the clipboard is attached instead of pasted. `true` if one was.
    pub(super) fn paste_image(&mut self, key: &ChannelKey, cx: &mut Context<Self>) -> bool {
        let Ok(mut clipboard) = arboard::Clipboard::new() else {
            return false;
        };
        let Ok(image) = clipboard.get_image() else {
            return false;
        };
        let Some(buffer) = image::RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.into_owned(),
        ) else {
            self.error("could not read the pasted image", cx);
            return true;
        };
        let dir = crate::open::paste_dir();
        self.next_upload += 1;
        let path = dir.join(format!(
            "paste-{}-{}.png",
            std::process::id(),
            self.next_upload
        ));
        // Encoding a large screenshot takes long enough to be felt on the UI thread.
        let key = key.clone();
        let written = path.clone();
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    buffer.save(&written).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |this, saved, _, cx| match saved {
                Ok(()) => this.start_upload(&key, path, Some("pasted image.png".into()), cx),
                Err(err) => this.error(format!("could not keep the pasted image: {err}"), cx),
            },
        );
        true
    }

    /// Downloads an attachment into the Downloads folder (never overwriting a file).
    pub fn download(&self, inst: InstanceId, attachment: Attachment, cx: &mut Context<Self>) {
        let core = self.core.clone();
        let (folder, ask) = (
            self.config.general.download_dir.clone(),
            self.config.general.ask_where_to_save,
        );
        self.run_status(cx, async move {
            let result: Result<String, String> = async {
                let chosen = if ask {
                    let name = crate::open::safe_filename(&attachment.filename);
                    let Some(file) = rfd::AsyncFileDialog::new()
                        .set_file_name(name)
                        .save_file()
                        .await
                    else {
                        return Ok("Download cancelled".to_owned());
                    };
                    Some(file.path().to_path_buf())
                } else {
                    None
                };
                let bytes = core.download(&inst, &attachment).await?;
                let dir = if folder.is_empty() {
                    crate::open::downloads_dir()
                } else {
                    std::path::PathBuf::from(folder)
                };
                let path = tokio::task::spawn_blocking(move || match chosen {
                    Some(path) => std::fs::write(&path, &bytes).map(|()| path),
                    None => crate::open::save_new(&dir, &attachment.filename, &bytes),
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
                Ok(format!("Saved to {}", path.display()))
            }
            .await;
            result.map_err(|e| format!("download failed: {e}"))
        });
    }

    /// Takes one file off a message; the message goes too when nothing is left of it.
    pub fn remove_attachment(
        &self,
        inst: InstanceId,
        channel: Id,
        message: Id,
        attachment: Id,
        cx: &mut Context<Self>,
    ) {
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.remove_attachment(&inst, channel, message, attachment)
                    .await
            },
            |this, result, _, cx| {
                if let Err(err) = result {
                    this.error(format!("could not remove: {err}"), cx);
                }
            },
        );
    }

    /// A file card's download button.
    pub fn download_button(
        &self,
        inst: &InstanceId,
        a: &Attachment,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let (inst, attachment) = (inst.clone(), a.clone());
        crate::ui::tip_button(
            self,
            SharedString::from(format!("file-download-{}", a.id)),
            assets::DOWNLOAD,
            "Download",
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.download(inst.clone(), attachment.clone(), cx);
        }))
    }
}
