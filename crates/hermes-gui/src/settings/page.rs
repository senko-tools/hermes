//! Opening and closing settings, the level meter, sessions, devices, sliders and themes.

use super::*;

impl Shell {
    /// Moves the level meter at [`METER_FPS`] while it shows, redrawing only when the bar would
    /// visibly move: a window redraw is what costs, not the meter.
    pub fn start_meter_timer(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut wait = std::time::Duration::from_secs_f32(1.0 / METER_FPS);
            loop {
                cx.background_executor().timer(wait).await;
                let alive = this.update(cx, |this, cx| {
                    let showing = this
                        .settings
                        .as_ref()
                        .is_some_and(|page| page.tab == Tab::Voice);
                    let now = std::time::Instant::now();
                    let seconds = now.duration_since(this.meter_drawn).as_secs_f32();
                    this.meter_drawn = now;
                    if !showing {
                        return false;
                    }
                    let next = meter_step(this.meter_shown, this.input_level, seconds);
                    let moved = (next - this.meter_shown).abs();
                    this.meter_shown = next;
                    let opened = this.meter_open != this.input_open;
                    this.meter_open = this.input_open;
                    if moved >= METER_STEP_DB || opened {
                        cx.notify();
                    }
                    true
                });
                let Ok(showing) = alive else {
                    return;
                };
                // Hardly anything to do while the Voice tab is closed.
                wait = std::time::Duration::from_secs_f32(if showing {
                    1.0 / METER_FPS
                } else {
                    0.25
                });
            }
        })
        .detach();
    }

    /// Opens the settings over the whole window.
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let voice = self.config.voice.clone();
        let font = self.config.font.clone();
        let text_field = |value: String,
                          placeholder: &'static str,
                          window: &mut Window,
                          cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(placeholder);
                input.set_value(value, window, cx);
                input
            })
        };
        let mut installed = cx.text_system().all_font_names();
        installed.sort();
        installed.dedup();
        let mut font_select = |current: &str, field: fn(&mut ClientConfig) -> &mut String| {
            let (choices, selected) = font_choices(&installed, current);
            let state =
                cx.new(|cx| SelectState::new(choices, Some(IndexPath::new(selected)), window, cx));
            let events = cx.subscribe(
                &state,
                move |this, _, event: &SelectEvent<Vec<DeviceChoice>>, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        let value = value.clone();
                        this.set_config(|c| *field(c) = value, cx);
                        this.reload_theme(cx);
                        this.remeasure_chats();
                    }
                },
            );
            (state, events)
        };
        let (font_input, font_events) = font_select(&font.family, |c| &mut c.font.family);
        let (mono_input, mono_events) = font_select(&font.mono_family, |c| &mut c.font.mono_family);
        let time_format = text_field(
            self.config.buffer.timestamp_format.clone(),
            "%H:%M",
            window,
            cx,
        );
        let date_format = text_field(
            self.config.buffer.date_format.clone(),
            "%B %e, %Y",
            window,
            cx,
        );
        let ptt_key = text_field(voice.ptt_key.clone(), "e.g. Mouse4", window, cx);
        let search = text_field(String::new(), "Search settings", window, cx);
        let mut events = vec![
            cx.subscribe_in(&search, window, |this, _, event, window, cx| match event {
                InputEvent::Change => cx.notify(),
                // Enter opens the first thing found.
                InputEvent::PressEnter { .. } => {
                    if let Some((tab, found)) = this.settings_found(cx).into_iter().next() {
                        this.open_found(tab, found, window, cx);
                    }
                }
                _ => {}
            }),
            cx.subscribe_in(&date_format, window, |this, input, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().to_string();
                    this.set_pattern(Pattern::Date, value, true, window, cx);
                    this.success("Date format saved", cx);
                }
            }),
            font_events,
            mono_events,
            cx.subscribe_in(&time_format, window, |this, input, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().to_string();
                    this.set_pattern(Pattern::Time, value, true, window, cx);
                    this.success("Time format saved", cx);
                }
            }),
            cx.subscribe_in(&ptt_key, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let text = input.read(cx).value().trim().to_owned();
                    if !text.is_empty() && crate::ptt::virtual_keys(&text).is_none() {
                        this.error(format!("{text:?} is not a key Hermes knows"), cx);
                    } else {
                        this.set_config(|c| c.voice.ptt_key = text, cx);
                        this.success("Push-to-talk key saved", cx);
                    }
                }
            }),
        ];
        let table = [
            (SliderKind::FontSize, 8., 32., 1., f32::from(font.size)),
            (
                SliderKind::UiScale,
                theme::MIN_SCALE as f32,
                theme::MAX_SCALE as f32,
                0.05,
                font.ui_scale as f32,
            ),
            (
                SliderKind::GroupMinutes,
                0.,
                30.,
                1.,
                self.config.buffer.group_minutes.min(30) as f32,
            ),
            (
                SliderKind::InputVolume,
                0.,
                hermes_client_core::audio::MAX_INPUT_VOLUME,
                0.05,
                voice.input_volume as f32,
            ),
            (
                SliderKind::OutputVolume,
                0.,
                hermes_client_core::audio::MAX_VOLUME,
                0.05,
                voice.output_volume as f32,
            ),
            (
                SliderKind::Vad,
                METER_MIN,
                0.,
                1.,
                (voice.vad_sensitivity as f32).clamp(METER_MIN, 0.),
            ),
            (
                SliderKind::ReleaseDelay,
                0.,
                1000.,
                10.,
                voice.ptt_release_delay_ms as f32,
            ),
            (
                SliderKind::VadHold,
                100.,
                1000.,
                10.,
                (voice.vad_hold_ms as f32).clamp(100., 1000.),
            ),
            (
                SliderKind::Ducking,
                3.,
                30.,
                1.,
                (voice.ducking_db as f32).clamp(3., 30.),
            ),
            (
                SliderKind::AgcTarget,
                -30.,
                -6.,
                1.,
                (voice.agc_target_db as f32).clamp(-30., -6.),
            ),
            (
                SliderKind::SoundsVolume,
                0.,
                1.,
                0.05,
                voice.sounds_volume as f32,
            ),
            (
                SliderKind::MediaVolume,
                0.,
                1.,
                0.05,
                (voice.media_volume as f32).clamp(0., 1.),
            ),
            (
                SliderKind::ScrollSpeed,
                0.5,
                3.,
                0.05,
                self.config.general.scroll_speed as f32,
            ),
            (
                SliderKind::HintDelay,
                0.,
                2000.,
                50.,
                (self.config.general.hint_scroll_delay_ms as f32).clamp(0., 2000.),
            ),
        ];
        let mut sliders = Vec::new();
        for (kind, min, max, step, value) in table {
            let state = cx.new(|_| {
                SliderState::new()
                    .min(min)
                    .max(max)
                    .step(step)
                    .default_value(value)
            });
            events.push(cx.subscribe_in(
                &state,
                window,
                move |this, _, event: &SliderEvent, window, cx| match event {
                    SliderEvent::Change(value) => {
                        this.slider_moved(kind, value.end(), cx);
                    }
                    SliderEvent::Release(value) => {
                        this.slider_moved(kind, value.end(), cx);
                        this.slider_released(kind, window, cx);
                    }
                },
            ));
            sliders.push((kind, state));
        }
        self.settings = Some(SettingsPage {
            tab: self.settings_tab,
            account_inst: None,
            sessions: Remote::Idle,
            sessions_load: 0,
            capturing_key: None,
            two_factor: None,
            admin_users: Remote::Idle,
            admin_log: Remote::Idle,
            admin_settings: None,
            device_selects: None,
            devices_listing: false,
            device_poll: 0,
            screen_select: None,
            mic_test: false,
            fine_timers: None,
            font: font_input,
            mono_font: mono_input,
            time_format,
            date_format,
            search,
            ptt_key,
            sliders,
            _events: events,
        });
        let tab = self.settings_tab;
        self.settings_open_tab(tab, cx);
        cx.notify();
    }

    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.settings.take() {
            self.settings_tab = page.tab;
            if page.mic_test {
                self.core.set_mic_test(false);
            }
        }
        // Diagnostics still listening finish first; they turn the meter off themselves.
        let listening = self
            .diagnostics
            .as_ref()
            .is_some_and(crate::diagnostics::Run::listening);
        if !listening {
            self.core.set_level_meter(false);
        }
        cx.notify();
    }

    pub(crate) fn settings_open_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let Some(page) = &mut self.settings else {
            return;
        };
        page.tab = tab;
        reset_page_scroll();
        page.fine_timers = (tab == Tab::Voice).then(crate::desktop::window::FineTimers::new);
        // Diagnostics listen to the microphone for a moment of their own.
        let listening = self
            .diagnostics
            .as_ref()
            .is_some_and(crate::diagnostics::Run::listening);
        self.core.set_level_meter(tab == Tab::Voice || listening);
        match tab {
            Tab::Account => self.load_sessions(cx),
            Tab::Admin => self.load_admin(cx),
            Tab::Voice => {
                self.load_devices(cx);
                self.start_device_poll(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The instance the Account tab shows.
    pub(super) fn account_instance(&self) -> Option<InstanceId> {
        let ids = self.core.instance_ids();
        self.settings
            .as_ref()
            .and_then(|p| p.account_inst.clone())
            .filter(|i| ids.contains(i))
            .or_else(|| ids.first().cloned())
    }

    pub(super) fn load_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(inst) = self.account_instance() else {
            return;
        };
        let Some(page) = &mut self.settings else {
            return;
        };
        page.sessions = Remote::Loading;
        page.two_factor = None;
        page.sessions_load += 1;
        let load = page.sessions_load;
        let twofa_core = self.core.clone();
        let twofa_inst = inst.clone();
        self.run(
            cx,
            async move {
                let rest = twofa_core.rest(&twofa_inst).ok()?;
                rest.two_factor().await.ok().map(|s| s.enabled)
            },
            move |this, enabled, _, cx| {
                if let Some(page) = &mut this.settings
                    && page.sessions_load == load
                {
                    page.two_factor = enabled;
                }
                cx.notify();
            },
        );
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.rest(&inst)?
                    .sessions()
                    .await
                    .map_err(|e| e.to_string())
            },
            move |this, result, _, _| {
                // An older load (another instance picked since, or a reload after signing a
                // device out) is of no use any more.
                if let Some(page) = &mut this.settings
                    && page.sessions_load == load
                {
                    page.sessions = match result {
                        Ok(list) => Remote::Ready(list),
                        Err(err) => Remote::Failed(err),
                    };
                }
            },
        );
    }

    /// Lists the audio devices and screens in the background (it can take a moment).
    pub(super) fn load_devices(&mut self, cx: &mut Context<Self>) {
        self.list_audio_devices(cx);
        self.load_screens(cx);
    }

    /// Lists the audio devices again and brings the dropdowns up to date, keeping what is chosen
    /// (and an open menu open). Runs when the Voice tab opens, every [`DEVICE_POLL`] while it
    /// shows, and when a device dropdown is pressed: there is no Refresh button.
    pub(super) fn list_audio_devices(&mut self, cx: &mut Context<Self>) {
        let Some(page) = &mut self.settings else {
            return;
        };
        if page.devices_listing {
            return;
        }
        page.devices_listing = true;
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || core.list_audio_devices())
                    .await
                    .unwrap_or_default()
            },
            |this, devices, window, cx| {
                let Some(page) = &mut this.settings else {
                    return;
                };
                page.devices_listing = false;
                let Some(selects) = &mut page.device_selects else {
                    let selects = this.device_selects(&devices, window, cx);
                    if let Some(page) = &mut this.settings {
                        page.device_selects = Some(selects);
                    }
                    return;
                };
                if selects.listed == devices {
                    return;
                }
                let voice = &this.config.voice;
                for (state, names, current) in [
                    (&selects.input, &devices.inputs, &voice.input_device),
                    (&selects.output, &devices.outputs, &voice.output_device),
                ] {
                    let (choices, _) = device_choices(names, current);
                    state.update(cx, |state, cx| state.set_items(choices, window, cx));
                }
                selects.listed = devices;
            },
        );
    }

    /// Lists the audio devices every [`DEVICE_POLL`] while the Voice tab shows. Started each
    /// time the tab opens; an older timer stops by itself.
    pub(super) fn start_device_poll(&mut self, cx: &mut Context<Self>) {
        let Some(page) = &mut self.settings else {
            return;
        };
        page.device_poll += 1;
        let poll = page.device_poll;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DEVICE_POLL).await;
                let alive = this.update(cx, |this, cx| {
                    let showing = this
                        .settings
                        .as_ref()
                        .is_some_and(|page| page.tab == Tab::Voice && page.device_poll == poll);
                    if showing {
                        this.list_audio_devices(cx);
                    }
                    showing
                });
                if !matches!(alive, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }

    /// The screen dropdown of the Screen sharing tab, from the listed screens; made again
    /// whenever they have been listed.
    pub(crate) fn refresh_screen_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_none() || self.screen_list.is_empty() {
            return;
        }
        let current = self.config.voice.share_screen.clone();
        let choices: Vec<DeviceChoice> = self
            .screen_list
            .iter()
            .map(|s| DeviceChoice {
                value: s.value.clone(),
                name: s.label().into(),
            })
            .collect();
        let selected = choices.iter().position(|c| c.value == current).unwrap_or(0);
        let state =
            cx.new(|cx| SelectState::new(choices, Some(IndexPath::new(selected)), window, cx));
        let events = cx.subscribe(
            &state,
            |this, _, event: &SelectEvent<Vec<DeviceChoice>>, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    let value = value.clone();
                    this.set_config(|c| c.voice.share_screen = value, cx);
                }
            },
        );
        if let Some(page) = &mut self.settings {
            page.screen_select = Some((state, events));
        }
    }

    pub(super) fn device_selects(
        &self,
        devices: &AudioDevices,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DeviceSelects {
        let voice = &self.config.voice;
        let mut select = |names: &[String], current: &str, input: bool| {
            let (choices, selected) = device_choices(names, current);
            let state =
                cx.new(|cx| SelectState::new(choices, Some(IndexPath::new(selected)), window, cx));
            let events = cx.subscribe(
                &state,
                move |this, _, event: &SelectEvent<Vec<DeviceChoice>>, cx| {
                    let SelectEvent::Confirm(Some(value)) = event else {
                        return;
                    };
                    let value = value.clone();
                    this.change_voice(cx, move |voice| {
                        if input {
                            voice.input_device = value;
                        } else {
                            voice.output_device = value;
                        }
                    });
                },
            );
            (state, events)
        };
        let (input, input_events) = select(&devices.inputs, &voice.input_device, true);
        let (output, output_events) = select(&devices.outputs, &voice.output_device, false);
        DeviceSelects {
            input,
            output,
            listed: devices.clone(),
            _events: [input_events, output_events],
        }
    }

    pub(super) fn slider_moved(&mut self, kind: SliderKind, value: f32, cx: &mut Context<Self>) {
        let rounded = |v: f32| f64::from((v * 100.0).round() / 100.0);
        let voice = &mut self.config.voice;
        match kind {
            SliderKind::FontSize => self.config.font.size = value as u16,
            SliderKind::UiScale => self.ui_scale_draft = Some(rounded(value)),
            SliderKind::GroupMinutes => self.config.buffer.group_minutes = value as u32,
            SliderKind::InputVolume => voice.input_volume = rounded(value) as _,
            SliderKind::OutputVolume => voice.output_volume = rounded(value) as _,
            SliderKind::Vad => voice.vad_sensitivity = value as i32,
            SliderKind::ReleaseDelay => voice.ptt_release_delay_ms = value as u32,
            SliderKind::VadHold => voice.vad_hold_ms = value as u32,
            SliderKind::AgcTarget => voice.agc_target_db = value as i32,
            SliderKind::Ducking => voice.ducking_db = value as u32,
            SliderKind::SoundsVolume => voice.sounds_volume = rounded(value) as _,
            SliderKind::MediaVolume => voice.media_volume = rounded(value) as _,
            SliderKind::ScrollSpeed => self.config.general.scroll_speed = rounded(value) as _,
            SliderKind::HintDelay => self.config.general.hint_scroll_delay_ms = value as u32,
        }
        cx.notify();
    }

    pub(super) fn slider_released(
        &mut self,
        kind: SliderKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            SliderKind::FontSize => {
                let size = self.config.font.size;
                self.set_config(|c| c.font.size = size, cx);
                self.reload_theme(cx);
                self.remeasure_chats();
            }
            SliderKind::UiScale => {
                if let Some(scale) = self.ui_scale_draft.take() {
                    self.set_ui_scale(scale, window, cx);
                }
            }
            SliderKind::GroupMinutes => {
                let minutes = self.config.buffer.group_minutes;
                self.set_config(|c| c.buffer.group_minutes = minutes, cx);
            }
            SliderKind::MediaVolume => {
                let volume = (self.config.voice.media_volume * 1000.0).round() / 1000.0;
                self.set_config(|c| c.voice.media_volume = volume, cx);
            }
            SliderKind::HintDelay => {
                let ms = self.config.general.hint_scroll_delay_ms;
                self.set_config(|c| c.general.hint_scroll_delay_ms = ms, cx);
            }
            SliderKind::ScrollSpeed => {
                let speed = (self.config.general.scroll_speed * 100.0).round() / 100.0;
                self.set_config(|c| c.general.scroll_speed = speed, cx);
            }
            _ => {
                let voice = self.config.voice.clone();
                self.save_voice(&voice, cx);
            }
        }
    }

    /// Applies and saves a UI scale (rounded to whole percents, within the allowed range).
    pub fn set_ui_scale(&mut self, scale: f64, window: &mut Window, cx: &mut Context<Self>) {
        self.ui_scale_draft = None;
        let scale = ((scale * 100.0).round() / 100.0).clamp(theme::MIN_SCALE, theme::MAX_SCALE);
        self.set_config(|c| c.font.ui_scale = scale, cx);
        self.reload_theme(cx);
        window.set_rem_size(gpui_kit::px(16. * scale as f32));
        self.remeasure_chats();
        cx.notify();
    }

    /// Applies `[voice]` to the audio engine and writes it to `config.toml`.
    pub(super) fn save_voice(
        &mut self,
        voice: &hermes_client_core::config::Voice,
        cx: &mut Context<Self>,
    ) {
        let settings = hermes_client_core::voice::audio_settings(voice);
        if let Err(err) = self.core.set_audio_settings(settings) {
            self.error(format!("setting not saved: {err}"), cx);
        }
        self.apply_config(cx);
    }

    pub(super) fn change_voice(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut hermes_client_core::config::Voice),
    ) {
        let mut voice = self.config.voice.clone();
        change(&mut voice);
        self.save_voice(&voice, cx);
    }

    /// Loads the configured theme again (from disk if it is a user theme) and applies it.
    pub fn reload_theme(&mut self, cx: &mut Context<Self>) {
        let dir = self.core.paths().map(|p| p.dir.join(theme::THEMES_DIR));
        let loaded = Theme::load(&self.config.general.theme, dir.as_deref());
        self.themes = theme::available(dir.as_deref());
        self.theme_previews = self
            .themes
            .iter()
            .map(|name| {
                let preview = Theme::load(name, dir.as_deref());
                let failed = preview
                    .warnings
                    .into_iter()
                    .find(|w| w.contains("not loaded") || w.contains("not found"));
                (name.clone(), failed.map_or(Ok(preview.theme), Err))
            })
            .collect();
        theme::set_active(loaded.theme, &self.config.font);
        theme::apply_to_kit(cx);
        if let Some(warning) = loaded.warnings.into_iter().next() {
            self.warn(warning, cx);
        }
        cx.notify();
    }

    /// Copies the current theme to a new file in the themes folder, to edit it.
    pub(super) fn copy_theme(&mut self, cx: &mut Context<Self>) -> Result<String, String> {
        let dir = self
            .core
            .paths()
            .map(|p| p.dir.join(theme::THEMES_DIR))
            .ok_or("no config directory")?;
        let name = self.config.general.theme.clone();
        let text = match std::fs::read_to_string(dir.join(format!("{name}.toml"))) {
            Ok(text) => text,
            Err(_) => theme::BUILTIN
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, text)| (*text).to_owned())
                .ok_or_else(|| format!("theme `{name}` not found"))?,
        };
        let mut copy = format!("{name}-custom");
        let mut n = 2;
        while dir.join(format!("{copy}.toml")).exists() {
            copy = format!("{name}-custom-{n}");
            n += 1;
        }
        let path = dir.join(format!("{copy}.toml"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        self.reload_theme(cx);
        self.info(
            format!(
                "Copied to {}: edit it and the changes show up at once",
                path.display()
            ),
            cx,
        );
        Ok(copy)
    }

    pub(super) fn set_theme(&mut self, name: String, cx: &mut Context<Self>) {
        self.set_config(|c| c.general.theme = name, cx);
        self.reload_theme(cx);
    }

    /// Theme files changed on disk, or the config: redo the look.
    pub fn start_theme_watcher(&self, cx: &mut Context<Self>) {
        let Some(dir) = self.core.paths().map(|p| p.dir.join(theme::THEMES_DIR)) else {
            return;
        };
        // One slot: a reload already waiting covers every change after it, so a burst of file
        // events (an editor saving, a folder copied in) cannot pile up.
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let watcher = std::fs::create_dir_all(&dir)
            .map_err(notify::Error::io)
            .and_then(|()| {
                use notify::Watcher as _;
                let mut watcher =
                    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                        if res.is_ok_and(|e| !e.kind.is_access()) {
                            let _ = tx.try_send(());
                        }
                    })?;
                watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;
                Ok(watcher)
            });
        let watcher = match watcher {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!("theme hot reload unavailable: {err}");
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let _watcher = watcher;
            while rx.recv().await.is_some() {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                while rx.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.reload_theme(cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Version, system and connection state, for bug reports (no tokens or message content).
    pub(super) fn debug_info(&self) -> String {
        let mut out = format!(
            "Hermes {} ({} {})\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        for inst in self.core.instance_ids() {
            out.push_str(&format!("instance {inst}: {:?}\n", self.core.status(&inst)));
        }
        out.push_str(&format!("voice: {:?}\n", self.core.voice_status()));
        out.push_str(&format!(
            "theme {}, font size {}, ui scale {}\n",
            self.config.general.theme, self.config.font.size, self.config.font.ui_scale
        ));
        if let Some(paths) = self.core.paths() {
            out.push_str(&format!("config dir: {}\n", paths.dir.display()));
        }
        out
    }

    pub(super) fn revoke_sessions(&mut self, ids: Vec<Id>, cx: &mut Context<Self>) {
        let Some(inst) = self.account_instance() else {
            return;
        };
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let rest = core.rest(&inst)?;
                for id in ids {
                    rest.delete_session(id).await?;
                }
                Ok::<(), String>(())
            },
            |this, result, _, cx| match result {
                Ok(()) => this.load_sessions(cx),
                Err(err) => this.error(format!("could not sign out: {err}"), cx),
            },
        );
    }

    pub(super) fn set_privacy(&mut self, req: UpdatePrivacyRequest, cx: &mut Context<Self>) {
        if let Some(inst) = self.account_instance() {
            self.call_status(
                &inst,
                "Privacy saved",
                move |r| async move { r.update_privacy(&req).await },
                cx,
            );
        }
    }

    // ---- drawing ------------------------------------------------------------------------------
}
