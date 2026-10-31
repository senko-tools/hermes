//! Hermes dev tool: headless client that joins voice and plays/records WAV files.
//!
//! ```text
//! hermes-voicebot --server hermes.senko.tools --user bot1 --password … --join General \
//!     --play test.wav --record out.wav
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Parser;
use hermes_client_core::VoiceStatus;
use hermes_client_core::address::{Fingerprint, HostPort};
use hermes_client_core::audio::SAMPLE_RATE;
use hermes_voicebot::{Bot, Login, signal, to_48k, wav};

#[derive(Parser)]
#[command(about = "Headless Hermes client that joins voice and plays/records WAV files")]
struct Args {
    /// Instance address: `host`, `host:port` or an `https://` URL.
    #[arg(long)]
    server: String,
    #[arg(long)]
    user: String,
    #[arg(long, env = "HERMES_BOT_PASSWORD")]
    password: String,
    /// Create the account instead of signing in.
    #[arg(long)]
    register: bool,
    /// Invite code to register with.
    #[arg(long, requires = "register")]
    invite: Option<String>,
    /// Pin the server certificate (`sha256:…` or `3F:A9:…`); otherwise a self-signed
    /// certificate is trusted on first use.
    #[arg(long)]
    fingerprint: Option<String>,
    /// Voice channel name (any space) or ID (also a DM).
    #[arg(long)]
    join: String,
    /// WAV file to play into the room.
    #[arg(long, conflicts_with = "tone")]
    play: Option<PathBuf>,
    /// Play tone bursts (200 ms on, 300 ms off) instead of a file.
    #[arg(long)]
    tone: bool,
    /// Play the file (or the tones) forever.
    #[arg(long = "loop")]
    looped: bool,
    /// Write what the bot hears (48 kHz mono 16-bit) to this file.
    #[arg(long)]
    record: Option<PathBuf>,
    /// Stay this many seconds (default: until the file has played plus 2 s, or Ctrl+C).
    #[arg(long)]
    duration: Option<f32>,
    /// Keep RNNoise on (off by default: it suppresses synthetic tones).
    #[arg(long)]
    denoise: bool,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // The probe of a self-signed certificate makes the platform verifier log errors.
                .unwrap_or_else(|_| "warn,rustls_platform_verifier=off".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), String> {
    let server = HostPort::parse(&args.server).map_err(|e| e.to_string())?;
    let fingerprint = args
        .fingerprint
        .as_deref()
        .map(Fingerprint::parse)
        .transpose()
        .map_err(|e| e.to_string())?;
    let samples = match (&args.play, args.tone) {
        (Some(path), _) => {
            let file = wav::read(path)?;
            Some(to_48k(&file.mono(), file.rate))
        }
        (None, true) => Some(signal::bursts(1, 200, 300)),
        (None, false) => None,
    };
    let login = Login {
        server,
        fingerprint,
        username: args.user.clone(),
        password: args.password.clone(),
        register: args.register.then(|| args.invite.clone()),
    };
    let bot = Bot::connect(&login, args.denoise).await?;
    if let Some(samples) = &samples {
        bot.play(
            samples.clone(),
            args.looped || (args.tone && args.duration.is_some()),
        );
    }
    let channel = bot.join(&args.join).await?;
    eprintln!("joined voice channel {channel} as {}", args.user);

    let started = Instant::now();
    let limit = args.duration.map(Duration::from_secs_f32).or_else(|| {
        samples
            .as_ref()
            .filter(|_| !args.looped)
            .map(|s| Duration::from_secs_f32(s.len() as f32 / SAMPLE_RATE as f32 + 2.0))
    });
    let mut report = tokio::time::interval(Duration::from_secs(5));
    report.tick().await;
    loop {
        if limit.is_some_and(|l| started.elapsed() >= l) {
            break;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = report.tick() => match bot.status() {
                VoiceStatus::Connected { ping_ms, loss_pct } => {
                    eprintln!("connected: ping {ping_ms} ms, loss {loss_pct:.1} %");
                }
                VoiceStatus::Disconnected => return Err("voice disconnected".into()),
                other => eprintln!("{other:?}"),
            },
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    bot.leave().await;
    if let Some(path) = &args.record {
        let (samples, _) = bot.recording();
        wav::write(
            path,
            &wav::Wav {
                rate: SAMPLE_RATE,
                channels: 1,
                samples,
            },
        )?;
        eprintln!("recorded {}", path.display());
    }
    Ok(())
}
