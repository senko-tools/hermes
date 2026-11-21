//! Voice and audio.

use super::*;

impl Shell {
    pub(super) fn tab_voice(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.config.voice;
        let device_select = |state: Option<&Entity<SelectState<Vec<DeviceChoice>>>>| {
            state.map_or_else(
                || muted("Listing devices…"),
                |state| Select::new(state).cursor_pointer().into_any_element(),
            )
        };
        let selects = page.device_selects.as_ref();
        // Device names are long: their dropdowns are wide.
        const DEVICE_W: f32 = 340.;
        // Sliders on the right; volumes run the card's width under their name.
        const SLIDER_W: f32 = 240.;
        note_part("Refresh devices", "Device list", "Microphone");
        let devices = group_with(
            "Devices",
            Button::new("devices-refresh")
                .cursor_pointer()
                .small()
                .label("Refresh devices")
                .on_click(cx.listener(|this, _, _, cx| this.load_devices(cx)))
                .into_any_element(),
            vec![
                setting_fit(
                    "Microphone",
                    "",
                    sized(DEVICE_W, device_select(selects.map(|s| &s.input))),
                ),
                setting_fit(
                    "Speakers",
                    "",
                    sized(DEVICE_W, device_select(selects.map(|s| &s.output))),
                ),
            ],
        );
        let volumes = group(
            "Volume",
            vec![
                setting_below(
                    "Microphone volume",
                    &hermes_client_core::audio::volume_label(v.input_volume as f32),
                    self.slider(page, SliderKind::InputVolume),
                ),
                setting_below(
                    "Output volume",
                    &hermes_client_core::audio::volume_label(v.output_volume as f32),
                    self.slider(page, SliderKind::OutputVolume),
                ),
            ],
        );
        let ptt_mode = v.mode == config::VoiceMode::PushToTalk;
        let mode_chips = [
            (
                config::VoiceMode::VoiceActivity,
                "Voice activity",
                assets::WAVEFORM,
            ),
            (
                config::VoiceMode::PushToTalk,
                "Push-to-talk",
                assets::KEYBOARD,
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name, glyph))| {
            crate::ui::segment_with(
                self,
                ("voice-mode", n),
                Some(crate::ui::icon_preview(glyph)),
                name,
                v.mode == value,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.change_voice(cx, move |voice| voice.mode = value);
            }))
        });
        // The bar moves smoothly; it is green while the microphone would go through, as the
        // input mode decides: above the line, or while push-to-talk is held.
        let level = self.meter_shown.clamp(METER_MIN, 0.);
        let open = self.meter_open;
        let fill = (level - METER_MIN) / -METER_MIN;
        let meter = div()
            .w_full()
            .h(px(6.))
            .rounded(px(3.))
            .bg(theme::hover())
            .child(
                div()
                    .h_full()
                    .rounded(px(3.))
                    .w(gpui_kit::relative(fill))
                    // Green while the voice would go through, blending so it does not flicker.
                    .with_spring(
                        "meter-open",
                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                            gpui_kit::SpringConfig::new(260., 34., 1.),
                        ))
                        .to(open),
                        |d, phase| {
                            d.bg(theme::mix(
                                theme::text_tertiary(),
                                theme::speaking(),
                                phase.0.clamp(0., 1.),
                            ))
                        },
                    ),
            );
        let mut input_rows = vec![setting_fit(
            "Input mode",
            "",
            crate::ui::segments().children(mode_chips),
        )];
        if ptt_mode {
            input_rows.push(setting_fit(
                "Push-to-talk key",
                if cfg!(windows) {
                    "e.g. Mouse4, F13, Ctrl+Space. Enter saves."
                } else {
                    "Only while Hermes is focused. Enter saves."
                },
                sized(SLIDER_W, Input::new(&page.ptt_key)),
            ));
            input_rows.push(setting_fit(
                "Release delay",
                &format!("{} ms", v.ptt_release_delay_ms),
                sized(SLIDER_W, self.slider(page, SliderKind::ReleaseDelay)),
            ));
        }
        if ptt_mode {
            // Sensitivity does nothing here: the key decides.
            input_rows.push(setting_below(
                "Microphone level",
                &format!("Green while your push-to-talk key is held · now {level:.0} dBFS"),
                meter,
            ));
        } else {
            input_rows.push(setting_below(
                "Voice activity sensitivity",
                &format!("Opens above {} dBFS · now {level:.0}", v.vad_sensitivity),
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(6.))
                    .child(meter)
                    .child(self.slider(page, SliderKind::Vad)),
            ));
        }
        if !ptt_mode {
            input_rows.push(setting_fit(
                "Stay open after speaking",
                &format!("{} ms, so the ends of words are not cut off", v.vad_hold_ms),
                sized(SLIDER_W, self.slider(page, SliderKind::VadHold)),
            ));
        }
        let mic_test = page.mic_test;
        input_rows.push(setting_fit(
            "Mic test",
            "Hear yourself (use headphones).",
            Button::new("mic-test")
                .cursor_pointer()
                .label(if mic_test { "Stop test" } else { "Start test" })
                .when(mic_test, |b| b.danger())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(page) = &mut this.settings {
                        page.mic_test = !mic_test;
                    }
                    this.core.set_mic_test(!mic_test);
                    cx.notify();
                })),
        ));
        let voice_switch = |id: &'static str,
                            on: bool,
                            set: fn(&mut hermes_client_core::config::Voice, bool),
                            cx: &mut Context<Self>| {
            Switch::new(id)
                .cursor_pointer()
                .checked(on)
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.change_voice(cx, move |voice| set(voice, on));
                }))
        };
        if !ptt_mode {
            let level_only = v.vad_level_only;
            input_rows.push(self.more(
                "voice-input",
                "More input settings",
                |cx| {
                    vec![setting_fit(
                        "Only loudness opens the microphone",
                        "Noise suppression's voice detection can't open it below the line.",
                        voice_switch(
                            "vad-level-only",
                            level_only,
                            |v, on| v.vad_level_only = on,
                            cx,
                        ),
                    )]
                },
                cx,
            ));
        }
        // What most people turn on or off; the fine-tuning folds away.
        let (fec, dtx, ducking_db, agc_target) =
            (v.opus_fec, v.opus_dtx, v.ducking_db, v.agc_target_db);
        let advanced = self.more(
            "voice-processing",
            "Advanced",
            |cx| {
                vec![
                    setting_fit(
                        "Lowered by",
                        &format!("{ducking_db} dB, while someone speaks"),
                        sized(SLIDER_W, self.slider(page, SliderKind::Ducking)),
                    ),
                    setting_fit(
                        "Automatic gain level",
                        &format!("Aims for {agc_target} dBFS. Higher is louder."),
                        sized(SLIDER_W, self.slider(page, SliderKind::AgcTarget)),
                    ),
                    setting_fit(
                        "Rebuild lost sound",
                        "Sends a little extra, so a dropped packet can be recovered.",
                        voice_switch("opus-fec", fec, |v, on| v.opus_fec = on, cx),
                    ),
                    setting_fit(
                        "Send nothing during silence",
                        "Saves bandwidth.",
                        voice_switch("opus-dtx", dtx, |v, on| v.opus_dtx = on, cx),
                    ),
                ]
            },
            cx,
        );
        let processing = group(
            "Processing",
            vec![
                setting_fit(
                    "Noise suppression",
                    if v.noise_suppression {
                        "Removes fans, keyboards and the like."
                    } else {
                        "Without it, headphones are recommended."
                    },
                    voice_switch(
                        "noise",
                        v.noise_suppression,
                        |v, on| v.noise_suppression = on,
                        cx,
                    ),
                ),
                setting_fit(
                    "Echo cancellation",
                    "For speakers or a laptop's own microphone: others don't hear \
                    themselves back. Not needed with headphones.",
                    voice_switch(
                        "echo",
                        v.echo_cancellation,
                        |v, on| v.echo_cancellation = on,
                        cx,
                    ),
                ),
                setting_fit(
                    "Automatic gain",
                    "Evens out how loud you are.",
                    voice_switch("gain", v.auto_gain, |v, on| v.auto_gain = on, cx),
                ),
                setting_fit(
                    "Lower other sound while someone speaks",
                    "Screen share sound, videos and audio files step back for voices.",
                    voice_switch("ducking", v.ducking, |v, on| v.ducking = on, cx),
                ),
                advanced,
            ],
        );
        let join_chips = [
            (config::JoinOn::DoubleClick, "Double-click"),
            (config::JoinOn::SingleClick, "Single click"),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (value, name))| {
            crate::ui::segment(self, ("join-on", n), name, v.join_on == value, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.set_config(|c| c.voice.join_on = value, cx);
                }),
            )
        })
        .collect::<Vec<_>>();
        let sounds = group(
            "Sounds and joining",
            vec![
                setting_fit(
                    "Play sounds",
                    "When people join or leave, and when you mute.",
                    voice_switch("sounds", v.sounds, |v, on| v.sounds = on, cx),
                ),
                setting_below(
                    "Sound volume",
                    &format!("{:.0} %", v.sounds_volume * 100.),
                    self.slider(page, SliderKind::SoundsVolume),
                ),
                setting_fit(
                    "Join a voice channel with",
                    "",
                    crate::ui::segments().children(join_chips),
                ),
            ],
        );
        page_body(
            "Voice & Audio",
            None,
            vec![
                devices,
                volumes,
                group("Input", input_rows),
                processing,
                sounds,
            ],
        )
    }
}
