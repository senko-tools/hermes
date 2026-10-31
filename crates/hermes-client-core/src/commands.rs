//! Client-side slash commands, shared by the GUI and the TUI (PLAN.md section 15.3).

use hermes_proto::models::PresenceStatus;

/// The appended text of `/shrug`; backslashes keep the markup parser from reading `_…_`.
pub const SHRUG: &str = r"¯\\\_(ツ)\_/¯";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// Not a command: send this text (also `//text`, which sends `/text`).
    Send(String),
    /// `/me waves` → sent as `_waves_`.
    Me(String),
    Join(String),
    Leave,
    Mute,
    Deafen,
    Nick(Option<String>),
    Dm(String),
    /// Create an invite to the current space.
    Invite,
    Status(PresenceStatus),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    Unknown(String),
    MissingArgument(&'static str),
    InvalidStatus(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::Unknown(c) => write!(f, "unknown command /{c} (try /help)"),
            CommandError::MissingArgument(usage) => write!(f, "usage: {usage}"),
            CommandError::InvalidStatus(s) => {
                write!(
                    f,
                    "unknown status {s:?}; use online, idle, dnd or invisible"
                )
            }
        }
    }
}

impl std::error::Error for CommandError {}

/// Every command with its usage, for autocomplete and help.
pub const COMMANDS: &[(&str, &str)] = &[
    ("me", "/me <action>"),
    ("shrug", "/shrug [text]"),
    ("join", "/join <voice channel>"),
    ("leave", "/leave"),
    ("mute", "/mute"),
    ("deafen", "/deafen"),
    ("nick", "/nick [name]"),
    ("dm", "/dm <user>"),
    ("invite", "/invite"),
    ("status", "/status <online|idle|dnd|invisible>"),
];

pub fn parse_status(s: &str) -> Option<PresenceStatus> {
    match s.trim().to_ascii_lowercase().as_str() {
        "online" => Some(PresenceStatus::Online),
        "idle" | "away" => Some(PresenceStatus::Idle),
        "dnd" => Some(PresenceStatus::Dnd),
        "invisible" | "offline" => Some(PresenceStatus::Invisible),
        _ => None,
    }
}

/// Parses what the user typed in the input box.
pub fn parse(input: &str) -> Result<SlashCommand, CommandError> {
    let Some(rest) = input.strip_prefix('/') else {
        return Ok(SlashCommand::Send(input.to_owned()));
    };
    if rest.starts_with('/') {
        return Ok(SlashCommand::Send(rest.to_owned()));
    }
    let (name, arg) = match rest.split_once(char::is_whitespace) {
        Some((name, arg)) => (name, arg.trim()),
        None => (rest, ""),
    };
    let required = |usage: &'static str| {
        if arg.is_empty() {
            Err(CommandError::MissingArgument(usage))
        } else {
            Ok(arg.to_owned())
        }
    };
    match name.to_ascii_lowercase().as_str() {
        "me" => Ok(SlashCommand::Me(required("/me <action>")?)),
        "shrug" => Ok(SlashCommand::Send(if arg.is_empty() {
            SHRUG.to_owned()
        } else {
            format!("{arg} {SHRUG}")
        })),
        "join" => Ok(SlashCommand::Join(required("/join <voice channel>")?)),
        "leave" => Ok(SlashCommand::Leave),
        "mute" => Ok(SlashCommand::Mute),
        "deafen" => Ok(SlashCommand::Deafen),
        "nick" => Ok(SlashCommand::Nick(
            (!arg.is_empty()).then(|| arg.to_owned()),
        )),
        "dm" => Ok(SlashCommand::Dm(
            required("/dm <user>")?.trim_start_matches('@').to_owned(),
        )),
        "invite" => Ok(SlashCommand::Invite),
        "status" => {
            let s = required("/status <online|idle|dnd|invisible>")?;
            parse_status(&s)
                .map(SlashCommand::Status)
                .ok_or(CommandError::InvalidStatus(s))
        }
        other => Err(CommandError::Unknown(other.to_owned())),
    }
}

impl SlashCommand {
    /// The message text for commands that send one.
    pub fn message_text(&self) -> Option<String> {
        match self {
            SlashCommand::Send(text) => Some(text.clone()),
            SlashCommand::Me(action) => Some(format!("_{action}_")),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse("hello").unwrap(), SlashCommand::Send("hello".into()));
        assert_eq!(
            parse("//not a command").unwrap(),
            SlashCommand::Send("/not a command".into())
        );
        assert_eq!(
            parse("/me waves").unwrap().message_text().unwrap(),
            "_waves_"
        );
        assert_eq!(parse("/shrug").unwrap().message_text().unwrap(), SHRUG);
        assert_eq!(
            parse("/shrug ok").unwrap().message_text().unwrap(),
            format!("ok {SHRUG}")
        );
        assert_eq!(
            parse("/join  Lounge ").unwrap(),
            SlashCommand::Join("Lounge".into())
        );
        assert_eq!(parse("/nick").unwrap(), SlashCommand::Nick(None));
        assert_eq!(
            parse("/nick Bob").unwrap(),
            SlashCommand::Nick(Some("Bob".into()))
        );
        assert_eq!(
            parse("/dm @alice").unwrap(),
            SlashCommand::Dm("alice".into())
        );
        assert_eq!(
            parse("/status DND").unwrap(),
            SlashCommand::Status(PresenceStatus::Dnd)
        );
        assert!(matches!(
            parse("/me"),
            Err(CommandError::MissingArgument(_))
        ));
        assert!(matches!(
            parse("/status busy"),
            Err(CommandError::InvalidStatus(_))
        ));
        assert!(matches!(
            parse("/frobnicate"),
            Err(CommandError::Unknown(_))
        ));
        assert_eq!(parse("/LEAVE").unwrap(), SlashCommand::Leave);
    }

    #[test]
    fn shrug_survives_the_markup_parser() {
        let spans = hermes_proto::markup::parse(SHRUG);
        let text: String = spans
            .iter()
            .map(|s| match s {
                hermes_proto::markup::Span::Text(t) => t.clone(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(text, "¯\\_(ツ)_/¯");
    }
}
