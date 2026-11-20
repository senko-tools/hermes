//! Commands typed into the server's console (its stdin: a terminal, or the Pterodactyl panel's
//! console): `help`, and `log` to choose what the log shows while the server runs.
//!
//! Stdin is read on its own thread, so a console that never types anything costs nothing and
//! never holds up shutdown. Without a console (stdin closed, as under systemd) it ends at once.

use std::io::BufRead;

use crate::config::{LogConfig, LogTopic};
use crate::logging::{self, Shown};

const HELP: &str = "\
Commands:
  help                      this list
  log                       what the log shows now
  log <topic> on|off        show or hide a topic; <topic> can also be all
  log level <level>         trace | debug | info | warn | error, or an EnvFilter string
                            such as \"info,hermes_server::voice=debug\"
  log reset                 back to the config file's log settings
Topics:
  connections   every TCP/TLS connection opened and closed, failed handshakes
  requests      every HTTP request: method, path, status, time
  gateway       gateway sessions starting, resuming and ending
  voice         joining and leaving voice channels
  auth          sign-ins, failed sign-ins, new accounts, sign-outs
Changes last until the server restarts; log.topics and log.level in the config set the start.";

/// A parsed console line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Help,
    Show,
    Topics(Vec<LogTopic>, bool),
    Level(String),
    Reset,
}

fn parse(line: &str) -> Result<Command, String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["help" | "?"] => Ok(Command::Help),
        ["log"] => Ok(Command::Show),
        ["log", "reset"] => Ok(Command::Reset),
        ["log", "level", ..] => {
            let level = line
                .trim()
                .splitn(3, char::is_whitespace)
                .nth(2)
                .unwrap_or("")
                .trim();
            if level.is_empty() {
                Err("Usage: log level <level>".into())
            } else {
                Ok(Command::Level(level.to_owned()))
            }
        }
        ["log", topic, switch] => {
            let on = match switch.to_ascii_lowercase().as_str() {
                "on" => true,
                "off" => false,
                _ => return Err("Usage: log <topic> on|off".into()),
            };
            let topics = if topic.eq_ignore_ascii_case("all") {
                LogTopic::ALL.to_vec()
            } else {
                vec![LogTopic::from_name(topic).ok_or_else(|| unknown_topic(topic))?]
            };
            Ok(Command::Topics(topics, on))
        }
        ["log", ..] => Err("Usage: log, log <topic> on|off, log level <level>, log reset".into()),
        [] => Err(String::new()),
        [word, ..] => Err(format!("Unknown command {word:?}; type help for the list")),
    }
}

fn unknown_topic(name: &str) -> String {
    let names: Vec<&str> = LogTopic::ALL.iter().map(|t| t.name()).collect();
    format!(
        "Unknown topic {name:?}; topics are {}, or all",
        names.join(", ")
    )
}

fn describe(shown: &Shown) -> String {
    let mut out = format!("Log level: {}\nTopics:", shown.level);
    for topic in LogTopic::ALL {
        let state = if shown.topics.contains(&topic) {
            "on"
        } else {
            "off"
        };
        out.push_str(&format!("\n  {:<12} {state}", topic.name()));
    }
    out
}

/// Runs one console line; the answer to print.
fn run(line: &str, defaults: &Shown) -> String {
    let command = match parse(line) {
        Ok(command) => command,
        Err(err) => return err,
    };
    let Some(mut shown) = logging::shown() else {
        return "Logging is not running".into();
    };
    match command {
        Command::Help => return HELP.into(),
        Command::Show => return describe(&shown),
        Command::Topics(topics, on) => {
            for topic in topics {
                if on {
                    shown.topics.insert(topic);
                } else {
                    shown.topics.remove(&topic);
                }
            }
        }
        Command::Level(level) => shown.level = level,
        Command::Reset => shown = defaults.clone(),
    }
    match logging::show(shown.clone()) {
        Ok(()) => describe(&shown),
        Err(err) => format!("Not changed: {err}"),
    }
}

/// Reads commands from stdin until it closes. `log` is the config's log section, which
/// `log reset` goes back to.
pub fn start(log: &LogConfig) {
    let defaults = Shown {
        level: log.level.clone(),
        topics: log.topics.iter().copied().collect(),
    };
    let spawned = std::thread::Builder::new()
        .name("console".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                let answer = run(&line, &defaults);
                if !answer.is_empty() {
                    println!("{answer}");
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("console commands are not available: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(parse("help"), Ok(Command::Help));
        assert_eq!(parse("  log "), Ok(Command::Show));
        assert_eq!(parse("log reset"), Ok(Command::Reset));
        assert_eq!(
            parse("log Voice ON"),
            Ok(Command::Topics(vec![LogTopic::Voice], true))
        );
        assert_eq!(
            parse("log all off"),
            Ok(Command::Topics(LogTopic::ALL.to_vec(), false))
        );
        assert_eq!(
            parse("log level info, hermes_server::voice=debug"),
            Ok(Command::Level("info, hermes_server::voice=debug".into()))
        );
        assert!(parse("log level").is_err());
        assert!(
            parse("log nonsense on")
                .unwrap_err()
                .contains("connections")
        );
        assert!(parse("log voice maybe").is_err());
        assert!(parse("stop").unwrap_err().contains("help"));
        assert_eq!(parse(""), Err(String::new()));
    }

    #[test]
    fn describe_lists_every_topic() {
        let text = describe(&Shown {
            level: "info".into(),
            topics: [LogTopic::Auth].into(),
        });
        assert!(text.contains("auth         on"), "{text}");
        assert!(text.contains("requests     off"), "{text}");
    }
}
