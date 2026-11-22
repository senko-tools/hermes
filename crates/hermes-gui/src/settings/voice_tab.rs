//! Voice and audio.

use super::*;
use crate::ui::{ButtonKind, DropWidth, Segment};

const VOICE_MODES: [(config::VoiceMode, &str); 2] = [
    (config::VoiceMode::VoiceActivity, "Voice activity"),
    (config::VoiceMode::PushToTalk, "Push-to-talk"),
];
const JOIN_ON: [(config::JoinOn, &str); 2] = [
    (config::JoinOn::DoubleClick, "Double-click"),
    (config::JoinOn::SingleClick, "Single click"),
];

/// How wide the sliders beside their name are.
const SLIDER_W: f32 = 240.;

impl Shell {
    pub(super) fn tab_voice(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.config.voice;
        let processing = self.voice_processing(page, cx);
        let sounds = section(
            "Sounds",
            "",
            vec![
                setting_fit(
                    "Play sounds",
                    "When people join or leave, and when you mute.",
                    self.voice_switch("sounds", v.sounds, |v, on| v.sounds = on, cx),
                ),
                setting_below(
                    "Sound volume",
                    &format!("{:.0} %", v.sounds_volume * 100.),
                    self.slider(page, SliderKind::SoundsVolume),
                ),
            ],
        );
        // TODO(settings rework): joining belongs on another page; it waits here at the bottom
        // until that page is reworked.
        let joining = section(
            "Joining",
            "",
            vec![setting_fit(
                "Join a voice channel with",
                "",
                crate::ui::segmented(
                    self,
                    "join-on",
                    JOIN_ON
                        .iter()
                        .map(|(_, name)| Segment::new(*name))
                        .collect(),
                    JOIN_ON.iter().position(|(j, _)| *j == v.join_on),
                    true,
                    |this, n, _, cx| {
                        let value = JOIN_ON[n].0;
                        this.set_config(|c| c.voice.join_on = value, cx);
                    },
                    cx,
                ),
            )],
        );
        page_body(
            "Voice & Audio",
            None,
            vec![
                self.voice_devices(page, cx),
                self.voice_input(page, cx),
                processing,
                sounds,
                joining,
            ],
        )
    }

    /// Input and output side by side, each device over its volume (as Discord has them).
    fn voice_devices(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.config.voice;
        let selects = page.device_selects.as_ref();
        // Pressing a dropdown lists the devices again: one plugged in a moment ago shows up
        // (the list also refreshes by itself while the tab shows).
        let device_select = |state: Option<&Entity<SelectState<Vec<DeviceChoice>>>>,
                             cx: &mut Context<Self>| {
            state.map_or_else(
                || muted("Listing devices…"),
                |state| {
                    div()
                        .w_full()
                        .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                            this.list_audio_devices(cx);
                        }))
                        .child(crate::ui::dropdown(state, DropWidth::Wide).w_full())
                        .into_any_element()
                },
            )
        };
        let column = |device: AnyElement, volume: AnyElement| {
            div()
                .flex()
                .flex_col()
                .child(device)
                .child(volume)
                .into_any_element()
        };
        let input = column(
            setting_below("Input", "", device_select(selects.map(|s| &s.input), cx)),
            setting_below(
                "Input volume",
                &hermes_client_core::audio::volume_label(v.input_volume as f32),
                self.slider(page, SliderKind::InputVolume),
            ),
        );
        let output = column(
            setting_below("Output", "", device_select(selects.map(|s| &s.output), cx)),
            setting_below(
                "Output volume",
                &hermes_client_core::audio::volume_label(v.output_volume as f32),
                self.slider(page, SliderKind::OutputVolume),
            ),
        );
        section("Devices", "", vec![setting_pair(input, output)])
    }

    /// Input mode, the level bar with the mic test beside it, and what opens the microphone.
    fn voice_input(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.config.voice;
        let ptt_mode = v.mode == config::VoiceMode::PushToTalk;
        let mode = crate::ui::radio_list(
            self,
            "voice-mode",
            VOICE_MODES
                .iter()
                .map(|(_, name)| (SharedString::from(*name), None))
                .collect(),
            VOICE_MODES.iter().position(|(m, _)| *m == v.mode),
            |this, n, _, cx| {
                let value = VOICE_MODES[n].0;
                this.change_voice(cx, move |voice| voice.mode = value);
            },
            cx,
        );
        let mut rows = vec![setting_below("Input mode", "", mode)];
        if ptt_mode {
            rows.push(setting_fit(
                "Push-to-talk key",
                if cfg!(windows) {
                    "e.g. Mouse4, F13, Ctrl+Space. Enter saves."
                } else {
                    "Only while Hermes is focused. Enter saves."
                },
                sized(SLIDER_W, Input::new(&page.ptt_key)),
            ));
            rows.push(setting_fit(
                "Release delay",
                &format!("{} ms", v.ptt_release_delay_ms),
                sized(SLIDER_W, self.slider(page, SliderKind::ReleaseDelay)),
            ));
        }
        let level = self.meter_shown.clamp(METER_MIN, 0.);
        // Automatic gain sits on the name's line, over the bar it changes.
        const LEVEL_ROW: &str = "Voice activity sensitivity";
        const PTT_LEVEL_ROW: &str = "Microphone level";
        let level_row = if ptt_mode { PTT_LEVEL_ROW } else { LEVEL_ROW };
        note_part("Automatic gain", "Evens out how loud you are.", level_row);
        note_part("Mic test", "Hear yourself (use headphones).", level_row);
        let auto_gain = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(theme::secondary_size())
            .text_color(theme::text_secondary())
            .child("Automatic gain")
            .child(self.voice_switch("gain", v.auto_gain, |v, on| v.auto_gain = on, cx));
        // Sensitivity does nothing with push-to-talk: the key decides.
        let (hint, slider) = if ptt_mode {
            (
                format!("Green while your push-to-talk key is held · now {level:.0} dBFS"),
                None,
            )
        } else {
            (
                format!("Opens above {} dBFS · now {level:.0}", v.vad_sensitivity),
                Some(self.slider(page, SliderKind::Vad)),
            )
        };
        rows.push(setting_split(
            level_row,
            &hint,
            auto_gain,
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(self.mic_test_button(page, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.))
                        .child(self.level_meter(level))
                        .children(slider),
                ),
        ));
        if !ptt_mode {
            let (level_only, hold) = (v.vad_level_only, v.vad_hold_ms);
            rows.push(self.more(
                "voice-input",
                "More input settings",
                |cx| {
                    vec![
                        setting_fit(
                            "Stay open after speaking",
                            &format!("{hold} ms, so the ends of words are not cut off"),
                            sized(SLIDER_W, self.slider(page, SliderKind::VadHold)),
                        ),
                        setting_fit(
                            "Only loudness opens the microphone",
                            "Noise suppression's voice detection can't open it below the line.",
                            self.voice_switch(
                                "vad-level-only",
                                level_only,
                                |v, on| v.vad_level_only = on,
                                cx,
                            ),
                        ),
                    ]
                },
                cx,
            ));
        }
        section("Input", "", rows)
    }

    /// "Mic Test": hear yourself. Filled with the accent while it runs.
    fn mic_test_button(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let mic_test = page.mic_test;
        crate::ui::button(
            self,
            "mic-test",
            "Mic Test",
            if mic_test {
                ButtonKind::Primary
            } else {
                ButtonKind::Secondary
            },
            true,
            move |this, _, cx| {
                if let Some(page) = &mut this.settings {
                    page.mic_test = !mic_test;
                }
                this.core.set_mic_test(!mic_test);
                cx.notify();
            },
            cx,
        )
        .into_any_element()
    }

    /// The input level bar. It moves smoothly and is green while the microphone would go
    /// through, as the input mode decides: above the line, or while push-to-talk is held.
    fn level_meter(&self, level: f32) -> AnyElement {
        let fill = (level - METER_MIN) / -METER_MIN;
        div()
            .w_full()
            .h(px(6.))
            .rounded(px(3.))
            .bg(theme::hover())
            .child(
                div()
                    .h_full()
                    .rounded(px(3.))
                    .w(gpui_kit::relative(fill))
                    // Blending, so it does not flicker.
                    .with_spring(
                        "meter-open",
                        gpui_kit::SpringAnimation::new(crate::theme::paced(
                            gpui_kit::SpringConfig::new(260., 34., 1.),
                        ))
                        .to(self.meter_open),
                        |d, phase| {
                            d.bg(theme::mix(
                                theme::text_tertiary(),
                                theme::speaking(),
                                phase.0.clamp(0., 1.),
                            ))
                        },
                    ),
            )
            .into_any_element()
    }

    fn voice_switch(
        &self,
        id: &'static str,
        on: bool,
        set: fn(&mut hermes_client_core::config::Voice, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        crate::ui::switch(
            self,
            id,
            on,
            true,
            move |this, _, cx| this.change_voice(cx, move |voice| set(voice, !on)),
            cx,
        )
        .into_any_element()
    }

    /// What is done to the voice before it is sent; the fine-tuning folds away.
    fn voice_processing(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let v = &self.config.voice;
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
                        self.voice_switch("opus-fec", fec, |v, on| v.opus_fec = on, cx),
                    ),
                    setting_fit(
                        "Send nothing during silence",
                        "Saves bandwidth.",
                        self.voice_switch("opus-dtx", dtx, |v, on| v.opus_dtx = on, cx),
                    ),
                ]
            },
            cx,
        );
        section(
            "Processing",
            "",
            vec![
                setting_fit(
                    "Noise suppression",
                    if v.noise_suppression {
                        "Removes fans, keyboards and the like."
                    } else {
                        "Without it, headphones are recommended."
                    },
                    self.voice_switch(
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
                    self.voice_switch(
                        "echo",
                        v.echo_cancellation,
                        |v, on| v.echo_cancellation = on,
                        cx,
                    ),
                ),
                setting_fit(
                    "Lower other sound while someone speaks",
                    "Screen share sound, videos and audio files step back for voices.",
                    self.voice_switch("ducking", v.ducking, |v, on| v.ducking = on, cx),
                ),
                advanced,
            ],
        )
    }
}
