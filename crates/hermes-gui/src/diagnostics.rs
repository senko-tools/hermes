//! The Diagnostics settings tab: tests every server this app is signed in to and this
//! computer, and shows each part as Working, Warning (might work, or works in a reduced way,
//! and the test cannot be sure) or Not working. The server checks are
//! `hermes_client_core::diagnostics`; the ones that need devices are here.

use crate::ui::ButtonKind;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, div};
use hermes_client_core::audio::Sound;
use hermes_client_core::diagnostics::{Check, Level, report};

use crate::assets::{self, icon};
use crate::settings::{group, page_body_with};
use crate::shell::Shell;
use crate::theme::{self, px};

/// How long the microphone is listened to.
const LISTEN: Duration = Duration::from_secs(3);
/// Below this the microphone counts as silent (dBFS).
const SILENT_DB: f32 = -60.0;

/// One run of the diagnostics.
pub struct Run {
    serial: u64,
    /// A heading and its checks; `None` while they are still being tested.
    groups: Vec<(String, Option<Vec<Check>>)>,
    /// The loudest microphone level heard while listening, and how many readings came.
    mic_peak: f32,
    mic_readings: u32,
    listening: bool,
}

impl Run {
    #[cfg(test)]
    pub fn groups(&self) -> &[(String, Option<Vec<Check>>)] {
        &self.groups
    }

    /// Whether the microphone is still being listened to.
    pub fn listening(&self) -> bool {
        self.listening
    }

    pub fn finished(&self) -> bool {
        self.groups.iter().all(|(_, checks)| checks.is_some())
    }
}

/// The group of checks about this computer.
const THIS_COMPUTER: &str = "This computer";

impl Shell {
    /// Starts (or restarts) the diagnostics.
    pub fn run_diagnostics(&mut self, cx: &mut Context<Self>) {
        let serial = self.diagnostics.as_ref().map_or(0, |r| r.serial) + 1;
        let instances = self.core.instance_ids();
        let mut groups: Vec<(String, Option<Vec<Check>>)> = instances
            .iter()
            .map(|inst| (inst.to_string(), None))
            .collect();
        groups.push((THIS_COMPUTER.to_owned(), None));
        self.diagnostics = Some(Run {
            serial,
            groups,
            mic_peak: f32::NEG_INFINITY,
            mic_readings: 0,
            listening: true,
        });
        // The microphone is listened to while the servers are tested.
        self.core.set_level_meter(true);

        for (n, inst) in instances.into_iter().enumerate() {
            let core = self.core.clone();
            self.run(
                cx,
                async move { core.diagnose_instance(&inst).await },
                move |this, checks, _, cx| {
                    if let Some(run) = this.diagnostics.as_mut().filter(|r| r.serial == serial)
                        && let Some(group) = run.groups.get_mut(n)
                    {
                        group.1 = Some(checks);
                        cx.notify();
                    }
                },
            );
        }

        let core = self.core.clone();
        // Turned off, GitHub is not asked here either.
        let check_updates = self.config.general.check_for_updates;
        self.run(
            cx,
            async move {
                let updates = async {
                    if check_updates {
                        core.diagnose_updates().await
                    } else {
                        Check::new("Updates", Level::Warning, "Update checks are turned off")
                    }
                };
                let blocking = tokio::task::spawn_blocking(|| {
                    (
                        crate::screen::hardware_encoders(),
                        crate::media::ffmpeg::version(),
                    )
                });
                let (updates, ()) = tokio::join!(updates, tokio::time::sleep(LISTEN));
                let (encoders, ffmpeg) = blocking
                    .await
                    .unwrap_or_else(|e| (Err(e.to_string()), Err(e.to_string())));
                (updates, encoders, ffmpeg)
            },
            move |this, (updates, encoders, ffmpeg), _, cx| {
                let Some(run) = this.diagnostics.as_mut().filter(|r| r.serial == serial) else {
                    return;
                };
                run.listening = false;
                let (peak, readings) = (run.mic_peak, run.mic_readings);
                let voice_tab = this
                    .settings
                    .as_ref()
                    .is_some_and(|p| p.tab == crate::settings::Tab::Voice);
                this.core.set_level_meter(voice_tab);
                let devices = this.core.list_audio_devices();
                let mut checks = vec![
                    mic_check(devices.inputs.is_empty(), peak, readings),
                    speakers_check(devices.outputs.is_empty()),
                ];
                checks.extend(this.ptt_check());
                checks.push(encoder_check(encoders));
                checks.push(ffmpeg_check(ffmpeg));
                checks.push(updates);
                checks.push(this.core.diagnose_storage());
                if let Some(run) = this.diagnostics.as_mut()
                    && let Some(group) = run.groups.last_mut()
                {
                    group.1 = Some(checks);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// A microphone reading while the diagnostics listen.
    pub fn diagnostics_heard(&mut self, db: f32) {
        if let Some(run) = self.diagnostics.as_mut().filter(|r| r.listening) {
            run.mic_peak = run.mic_peak.max(db);
            run.mic_readings += 1;
        }
    }

    /// Push-to-talk, only when it is the input mode.
    fn ptt_check(&self) -> Option<Check> {
        const NAME: &str = "Push-to-talk";
        let v = &self.config.voice;
        if v.mode != hermes_client_core::config::VoiceMode::PushToTalk {
            return None;
        }
        let key = v.ptt_key.trim();
        Some(if key.is_empty() {
            Check::new(NAME, Level::NotWorking, "No key is set")
                .hint("Set one in Voice & Audio: nothing goes through without it.")
        } else if cfg!(windows) {
            Check::new(NAME, Level::Working, format!("Key: {key}"))
        } else {
            Check::new(
                NAME,
                Level::Warning,
                format!("Key: {key}, only while Hermes is focused"),
            )
        })
    }

    pub(crate) fn tab_diagnostics(&self, cx: &mut Context<Self>) -> AnyElement {
        let run = self.diagnostics.as_ref();
        let running = run.is_some_and(|r| !r.finished());
        let label = match run {
            None => "Run diagnostics",
            Some(_) if running => "Testing…",
            Some(_) => "Run again",
        };
        let copy_label = self.copy_label("diagnostics-copy", "Copy report");
        let buttons = div()
            .flex()
            .gap(px(8.))
            .child(crate::ui::button(
                self,
                "diagnostics-run",
                label,
                ButtonKind::Primary,
                !running,
                |this, _, cx| this.run_diagnostics(cx),
                cx,
            ))
            .child(crate::ui::button(
                self,
                "diagnostics-copy",
                copy_label,
                ButtonKind::Secondary,
                run.is_some_and(Run::finished),
                |this, _, cx| {
                    let Some(run) = &this.diagnostics else {
                        return;
                    };
                    let groups: Vec<(String, Vec<Check>)> = run
                        .groups
                        .iter()
                        .filter_map(|(h, c)| Some((h.clone(), c.clone()?)))
                        .collect();
                    this.copy_from("diagnostics-copy", report(&groups), "Report", cx);
                },
                cx,
            ))
            .child(crate::ui::button(
                self,
                "diagnostics-sound",
                "Play test sound",
                ButtonKind::Secondary,
                true,
                |this, _, _| this.core.play_sound(Sound::Join),
                cx,
            ))
            .into_any_element();
        let developer = self.config.general.developer_mode;
        // The developer switch has nothing to do with the tests: it goes under them.
        let developer_group = group(
            "Developer",
            vec![crate::settings::setting_fit(
                "Developer mode",
                "Menus also offer copying the IDs of servers, channels, messages and people.",
                crate::ui::switch(
                    self,
                    "developer-mode",
                    developer,
                    true,
                    move |this, _, cx| {
                        this.set_config(|c| c.general.developer_mode = !developer, cx);
                    },
                    cx,
                ),
            )],
        );
        let mut groups = Vec::new();
        if let Some(run) = run {
            for (heading, checks) in &run.groups {
                let rows = match checks {
                    None => vec![testing_row()],
                    Some(checks) => checks.iter().map(check_row).collect(),
                };
                groups.push(group(heading, rows));
            }
        }
        groups.push(developer_group);
        page_body_with(
            "Diagnostics",
            Some(buttons),
            Some(
                "Tests every server you are signed in to and this computer. Working: verified. \
                 Warning: might work, or works in a reduced way, and the test cannot be sure. \
                 Not working: it failed. The report holds no passwords or messages.",
            ),
            groups,
        )
    }
}

fn mic_check(no_device: bool, peak: f32, readings: u32) -> Check {
    const NAME: &str = "Microphone";
    if no_device {
        return Check::new(NAME, Level::NotWorking, "No microphone found")
            .hint("Plug one in, or check that Windows sees it.");
    }
    if readings == 0 {
        return Check::new(
            NAME,
            Level::NotWorking,
            "The microphone could not be opened",
        )
        .hint("Another program may hold it, or Windows privacy settings may block it.");
    }
    if peak < SILENT_DB {
        return Check::new(
            NAME,
            Level::Warning,
            format!("Only silence for {} s", LISTEN.as_secs()),
        )
        .hint(
            "Muted, a very quiet room, or blocked by Windows privacy settings (Microphone access \
             for desktop apps). Say something and run again.",
        );
    }
    Check::new(
        NAME,
        Level::Working,
        format!("Hears sound (up to {peak:.0} dBFS)"),
    )
}

fn speakers_check(no_device: bool) -> Check {
    const NAME: &str = "Speakers";
    if no_device {
        return Check::new(NAME, Level::NotWorking, "No speakers or headphones found");
    }
    Check::new(NAME, Level::Warning, "Found, but a test cannot hear them")
        .hint("Press Play test sound: you should hear a short chime.")
}

fn encoder_check(encoders: Result<Vec<String>, String>) -> Check {
    const NAME: &str = "Screen share encoding";
    match encoders {
        Ok(names) if !names.is_empty() => Check::new(
            NAME,
            Level::Working,
            format!("Hardware: {}", names.join(", ")),
        ),
        Ok(_) => Check::new(NAME, Level::Warning, "Software only")
            .hint("Sharing works, but uses more CPU and may be less smooth."),
        Err(e) => Check::new(
            NAME,
            Level::Warning,
            format!("Could not look for hardware encoders: {e}"),
        ),
    }
}

fn ffmpeg_check(version: Result<String, String>) -> Check {
    const NAME: &str = "Media playback (FFmpeg)";
    match version {
        Ok(line) => Check::new(NAME, Level::Working, line),
        Err(_) => Check::new(NAME, Level::Warning, "FFmpeg was not found").hint(
            "Videos and audio files will not play in the chat. Reinstalling Hermes brings it back.",
        ),
    }
}

fn level_style(level: Level) -> (&'static str, gpui_kit::Rgba) {
    match level {
        Level::Working => (assets::CHECK, theme::success()),
        Level::Warning => (assets::ALERT, theme::warning()),
        Level::NotWorking => (assets::CLOSE, theme::danger()),
    }
}

fn check_row(check: &Check) -> AnyElement {
    let (glyph, color) = level_style(check.level);
    div()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(div().pt(px(2.)).child(icon(glyph, 18., color)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(theme::body_size())
                                .child(SharedString::from(check.name.clone())),
                        )
                        .child(
                            div()
                                .text_size(theme::caption_size())
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(color)
                                .child(check.level.label()),
                        ),
                )
                .child(
                    div()
                        .text_size(theme::secondary_size())
                        .text_color(theme::text_secondary())
                        .child(SharedString::from(check.detail.clone())),
                )
                .children(check.hint.as_ref().map(|hint| {
                    div()
                        .text_size(theme::secondary_size())
                        .text_color(theme::text_tertiary())
                        .child(SharedString::from(hint.clone()))
                })),
        )
        .into_any_element()
}

fn testing_row() -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(theme::secondary_size())
        .text_color(theme::text_tertiary())
        .child(icon(assets::LAN_PENDING, 18., theme::text_tertiary()))
        .child("Testing…")
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_microphone_is_judged_by_what_it_heard() {
        assert_eq!(mic_check(true, -20., 100).level, Level::NotWorking);
        assert_eq!(
            mic_check(false, f32::NEG_INFINITY, 0).level,
            Level::NotWorking
        );
        assert_eq!(mic_check(false, -80., 150).level, Level::Warning);
        assert_eq!(mic_check(false, -25., 150).level, Level::Working);
        assert_eq!(speakers_check(false).level, Level::Warning);
        assert_eq!(speakers_check(true).level, Level::NotWorking);
        assert_eq!(encoder_check(Ok(vec![])).level, Level::Warning);
        assert_eq!(
            encoder_check(Ok(vec!["NVIDIA H.264 Encoder MFT".into()])).level,
            Level::Working
        );
        assert_eq!(ffmpeg_check(Err("not found".into())).level, Level::Warning);
    }
}
