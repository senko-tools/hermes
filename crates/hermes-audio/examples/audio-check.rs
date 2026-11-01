//! Manual check of the audio engine on real devices.
//!
//! ```text
//! cargo run -p hermes-audio --example audio-check -- list
//! cargo run -p hermes-audio --example audio-check -- meter 10      # input level for 10 s
//! cargo run -p hermes-audio --example audio-check -- mic-test 15   # hear yourself (use headphones)
//! cargo run -p hermes-audio --example audio-check -- sounds        # every UI sound once
//! ```
//!
//! `HERMES_INPUT` / `HERMES_OUTPUT` pick devices by name; `HERMES_NO_DENOISE=1` turns noise
//! suppression off. Unplugging a device while `meter` or `mic-test` runs should print a
//! device change and keep going on the default device.

use std::time::Duration;

use hermes_audio::{AudioEvent, AudioSettings, Engine, Io, Sound, devices};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let secs = |i: usize| {
        args.get(i)
            .and_then(|s| s.parse().ok())
            .map_or(Duration::from_secs(10), Duration::from_secs)
    };
    let settings = AudioSettings {
        input_device: std::env::var("HERMES_INPUT").ok(),
        output_device: std::env::var("HERMES_OUTPUT").ok(),
        noise_suppression: std::env::var("HERMES_NO_DENOISE").is_err(),
        ..AudioSettings::default()
    };
    match args.first().map(String::as_str) {
        Some("list") | None => {
            let d = devices::list();
            println!("inputs (default {:?}):", d.default_input);
            for name in d.inputs {
                println!("  {name}");
            }
            println!("outputs (default {:?}):", d.default_output);
            for name in d.outputs {
                println!("  {name}");
            }
        }
        Some("meter") => {
            let engine = Engine::start(Io::Devices, settings, print_event);
            engine.set_level_meter(true);
            std::thread::sleep(secs(1));
        }
        Some("mic-test") => {
            let engine = Engine::start(Io::Devices, settings, print_event);
            engine.set_mic_test(true);
            engine.set_level_meter(true);
            std::thread::sleep(secs(1));
        }
        Some("sounds") => {
            let engine = Engine::start(Io::Devices, settings, print_event);
            for sound in Sound::ALL {
                println!("{sound:?}");
                engine.play_sound(sound);
                std::thread::sleep(Duration::from_millis(1800));
            }
        }
        Some(other) => eprintln!("unknown command {other:?}; use list, meter, mic-test or sounds"),
    }
}

fn print_event(event: AudioEvent) {
    match event {
        AudioEvent::InputLevel(db) => {
            let bars = ((db + 60.0).max(0.0) / 2.0) as usize;
            println!("{db:6.1} dBFS {}", "#".repeat(bars));
        }
        AudioEvent::Packet(_) => {}
        other => println!("{other:?}"),
    }
}
