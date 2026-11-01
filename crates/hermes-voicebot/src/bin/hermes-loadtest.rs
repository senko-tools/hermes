//! Load test (PLAN.md section 19, "50 bot clients chatting, 20 voicebots in one room"): registers
//! bot accounts with an invite, lets them chat in one text channel at a steady rate, optionally
//! puts voice bots playing tones into one voice channel, and measures how long messages take
//! from sending to arriving at another client.
//!
//! ```text
//! hermes-loadtest --server hermes.example.org --invite AbC123 --bots 50 --voice 20 \
//!     --duration 300
//! ```
//! Watch the server meanwhile, e.g. `systemd-cgtop` or `ps -o rss,pcpu -C hermes-server`.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use hermes_client_core::address::{Fingerprint, HostPort};
use hermes_client_core::proto::id::Id;
use hermes_client_core::proto::models::ChannelKind;
use hermes_client_core::store::Changed;
use hermes_client_core::{CoreEvent, Draft, now_ms};
use hermes_voicebot::{Bot, Login, signal};

#[derive(Parser, Clone)]
#[command(about = "Hermes load test: chat bots, voice bots and delivery latency")]
struct Args {
    /// Instance address: `host`, `host:port` or an `https://` URL.
    #[arg(long)]
    server: String,
    /// An invite to the space to test in. The first bot joins with it and creates a fresh
    /// invite for the others, so a one-use invite (even the first-start setup link) is enough.
    #[arg(long)]
    invite: String,
    /// Pin the server certificate; otherwise a self-signed one is trusted on first use.
    #[arg(long)]
    fingerprint: Option<String>,
    /// Chat bots.
    #[arg(long, default_value_t = 50)]
    bots: usize,
    /// Messages per bot per minute.
    #[arg(long, default_value_t = 6.0)]
    rate: f64,
    /// Voice bots playing tone bursts in `--voice-channel`.
    #[arg(long, default_value_t = 0)]
    voice: usize,
    /// Text channel to chat in.
    #[arg(long, default_value = "general")]
    channel: String,
    /// Voice channel for the voice bots.
    #[arg(long, default_value = "General")]
    voice_channel: String,
    /// Seconds to run after every bot is connected.
    #[arg(long, default_value_t = 60)]
    duration: u64,
    /// Bot usernames start with this (a random suffix keeps runs apart).
    #[arg(long, default_value = "lt")]
    prefix: String,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "error,rustls_platform_verifier=off".into()),
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

fn login(args: &Args, invite: &str, name: String) -> Result<Login, String> {
    Ok(Login {
        server: HostPort::parse(&args.server).map_err(|e| e.to_string())?,
        fingerprint: args
            .fingerprint
            .as_deref()
            .map(Fingerprint::parse)
            .transpose()
            .map_err(|e| e.to_string())?,
        username: name,
        password: "load test password".into(),
        register: Some(Some(invite.to_owned())),
    })
}

/// The text channel called `name` in any space of the bot.
fn text_channel(bot: &Bot, name: &str) -> Option<Id> {
    let store = bot.core.store();
    let s = store.instance(&bot.inst)?;
    s.spaces.values().find_map(|sp| {
        sp.channels
            .values()
            .find(|c| {
                c.kind == ChannelKind::Text
                    && c.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .map(|c| c.id)
    })
}

/// `p` in 0..=1 of sorted `values`.
fn percentile(values: &[u64], p: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let i = ((values.len() - 1) as f64 * p).round() as usize;
    values[i]
}

async fn run(args: Args) -> Result<(), String> {
    let run_id = format!("{:04x}", now_ms() as u64 & 0xffff);
    let name = |kind: &str, i: usize| format!("{}{run_id}{kind}{i}", args.prefix);

    let observer = Bot::connect(&login(&args, &args.invite, name("o", 0))?, false).await?;
    let channel = text_channel(&observer, &args.channel)
        .ok_or_else(|| format!("no text channel called {:?}", args.channel))?;
    let space = observer
        .core
        .store()
        .instance(&observer.inst)
        .and_then(|s| s.space_of(channel))
        .ok_or("the channel is not in a space")?;
    let invite = observer
        .core
        .rest(&observer.inst)
        .map_err(|e| e.to_string())?
        .create_invite(
            space,
            &hermes_client_core::proto::api::CreateInviteRequest {
                max_uses: Some((args.bots + args.voice) as u32),
                expires_in_secs: None,
            },
        )
        .await
        .map_err(|e| format!("cannot create an invite for the bots: {e}"))?
        .code;
    // A visible channel keeps its message window (hidden ones are trimmed).
    observer
        .core
        .set_visible_channels([(observer.inst.clone(), channel)]);
    observer
        .core
        .open_channel(&observer.inst, channel)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("observer connected; starting {} chat bots", args.bots);

    let started = Instant::now();
    let mut chat = Vec::new();
    for i in 0..args.bots {
        let login = login(&args, &invite, name("c", i))?;
        chat.push(tokio::spawn(
            async move { Bot::connect(&login, false).await },
        ));
        // Registering is rate limited per address; do not hit it all at once.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut bots = Vec::new();
    for task in chat {
        match task.await {
            Ok(Ok(bot)) => bots.push(bot),
            Ok(Err(err)) => eprintln!("chat bot failed: {err}"),
            Err(err) => eprintln!("chat bot task failed: {err}"),
        }
    }
    let mut voice = Vec::new();
    for i in 0..args.voice {
        let bot = Bot::connect(&login(&args, &invite, name("v", i))?, false).await?;
        bot.play(signal::bursts(1 + i % 5, 200, 300), true);
        match bot.join(&args.voice_channel).await {
            Ok(_) => voice.push(bot),
            Err(err) => eprintln!("voice bot {i} could not join: {err}"),
        }
    }
    eprintln!(
        "{} chat bots and {} voice bots connected in {:.1} s; running for {} s",
        bots.len(),
        voice.len(),
        started.elapsed().as_secs_f32(),
        args.duration
    );

    // Only messages of this run count (the channel's history is loaded too).
    let tag = format!("lt-{run_id}");
    let sent = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let interval = Duration::from_secs_f64(60.0 / args.rate.max(0.01));
    let deadline = Instant::now() + Duration::from_secs(args.duration);
    let mut senders = Vec::new();
    for (i, bot) in bots.iter().enumerate() {
        let core = bot.core.clone();
        let inst = bot.inst.clone();
        let (sent, failed) = (sent.clone(), failed.clone());
        let tag = tag.clone();
        // Spread the bots over the interval.
        let offset = interval.mul_f64(i as f64 / bots.len().max(1) as f64);
        senders.push(tokio::spawn(async move {
            tokio::time::sleep(offset).await;
            let mut tick = tokio::time::interval(interval);
            let mut seq = 0u64;
            loop {
                tick.tick().await;
                if Instant::now() >= deadline {
                    break;
                }
                seq += 1;
                let draft = Draft {
                    content: format!("{tag} {} {i} {seq}", now_ms()),
                    reply_to: None,
                    attachments: Vec::new(),
                };
                match core.send_message(&inst, channel, draft) {
                    Ok(_) => sent.fetch_add(1, Ordering::Relaxed),
                    Err(_) => failed.fetch_add(1, Ordering::Relaxed),
                };
            }
        }));
    }

    // The observer reads every new message and measures its delay.
    let mut latencies = Vec::new();
    let mut last_seen = Id(0);
    let mut events = observer.events;
    let end = deadline + Duration::from_secs(5);
    let mut report = Instant::now();
    while Instant::now() < end {
        let event = tokio::time::timeout(Duration::from_millis(500), events.recv()).await;
        let arrived = now_ms();
        if let Ok(Some(CoreEvent::StoreChanged {
            what: Changed::Messages(c),
            ..
        })) = event
            && c == channel
        {
            let store = observer.core.store();
            if let Some(window) = store
                .instance(&observer.inst)
                .and_then(|s| s.window(channel))
            {
                let seen = last_seen;
                for m in window.messages.iter().filter(|m| m.id > seen) {
                    let sent_at = m
                        .content
                        .strip_prefix(tag.as_str())
                        .and_then(|rest| rest.strip_prefix(' '))
                        .and_then(|rest| rest.split(' ').next())
                        .and_then(|t| t.parse::<i64>().ok());
                    if let Some(sent_at) = sent_at {
                        latencies.push((arrived - sent_at).max(0) as u64);
                    }
                    last_seen = m.id;
                }
            }
        }
        if report.elapsed() >= Duration::from_secs(10) {
            report = Instant::now();
            let voice_ok = voice
                .iter()
                .filter(|b| {
                    matches!(
                        b.status(),
                        hermes_client_core::VoiceStatus::Connected { .. }
                    )
                })
                .count();
            eprintln!(
                "sent {}, received {}, voice bots connected {voice_ok}/{}",
                sent.load(Ordering::Relaxed),
                latencies.len(),
                voice.len()
            );
        }
    }
    for task in senders {
        let _ = task.await;
    }
    for bot in &voice {
        bot.leave().await;
    }

    latencies.sort_unstable();
    let sent = sent.load(Ordering::Relaxed);
    println!("messages sent:      {sent}");
    println!("messages received:  {} (by the observer)", latencies.len());
    println!("send errors:        {}", failed.load(Ordering::Relaxed));
    println!(
        "delivery latency:   p50 {} ms, p95 {} ms, p99 {} ms, max {} ms",
        percentile(&latencies, 0.5),
        percentile(&latencies, 0.95),
        percentile(&latencies, 0.99),
        latencies.last().copied().unwrap_or(0)
    );
    if (latencies.len() as u64) < sent {
        return Err(format!(
            "{} messages did not arrive",
            sent - latencies.len() as u64
        ));
    }
    Ok(())
}
