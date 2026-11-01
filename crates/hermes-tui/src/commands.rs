//! The `:` command line (PLAN.md section 16.2). Slash commands typed in insert mode are parsed
//! by `hermes_client_core::commands`.

use hermes_client_core::commands::parse_status;
use hermes_client_core::proto::models::PresenceStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open a channel by name (`#` optional).
    Join(String),
    Dm(String),
    /// Join a voice room: a voice channel by name, or (empty) the ringing call / open channel.
    Voice(String),
    Leave,
    Mute,
    Deafen,
    Status(PresenceStatus),
    Upload(String),
    /// Open the n-th attachment (1-based) of the selected message.
    Open(usize),
    /// `:instance add <invite-or-address>`.
    AddInstance(String),
    /// `:instance remove [id]`.
    RemoveInstance(Option<String>),
    Login,
    Logout,
    /// `:invite`: create an invite to the current space.
    Invite,
    Retry,
    Set(String, String),
    Offline,
    Online,
    Help,
    Quit,
}

pub const HELP: &str = ":join <channel>  :dm <user>  :voice [channel]  :leave  :mute  :deafen  :status <s>  :upload <path>  :open <n>  \
:invite  :instance add <link|host>  :instance remove  :login  :logout  :set <key> <value>  \
:offline  :online  :retry  :quit";

pub fn parse(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let (name, arg) = match line.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (line, ""),
    };
    let need = |usage: &str| -> Result<String, String> {
        if arg.is_empty() {
            Err(format!("usage: {usage}"))
        } else {
            Ok(arg.to_owned())
        }
    };
    Ok(match name {
        "join" | "j" => Command::Join(need(":join <channel>")?.trim_start_matches('#').to_owned()),
        "dm" => Command::Dm(need(":dm <user>")?.trim_start_matches('@').to_owned()),
        "voice" | "v" => Command::Voice(arg.to_owned()),
        "leave" => Command::Leave,
        "mute" => Command::Mute,
        "deafen" => Command::Deafen,
        "status" => {
            let s = need(":status <online|idle|dnd|invisible>")?;
            Command::Status(parse_status(&s).ok_or_else(|| format!("unknown status {s:?}"))?)
        }
        "upload" => Command::Upload(need(":upload <path>")?),
        "open" => {
            let n = need(":open <attachment number>")?;
            Command::Open(
                n.parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or_else(|| format!("{n:?} is not an attachment number"))?,
            )
        }
        "instance" => match arg.split_once(char::is_whitespace) {
            Some(("add", target)) => Command::AddInstance(target.trim().to_owned()),
            Some(("remove", id)) => Command::RemoveInstance(Some(id.trim().to_owned())),
            None if arg == "remove" => Command::RemoveInstance(None),
            _ => {
                return Err(
                    "usage: :instance add <invite-or-address> | :instance remove [id]".into(),
                );
            }
        },
        "login" => Command::Login,
        "logout" => Command::Logout,
        "invite" => Command::Invite,
        "retry" => Command::Retry,
        "set" => {
            let rest = need(":set <key> <value>")?;
            let (key, value) = rest
                .split_once(char::is_whitespace)
                .ok_or("usage: :set <key> <value>")?;
            Command::Set(key.to_owned(), value.trim().to_owned())
        }
        "offline" => Command::Offline,
        "online" => Command::Online,
        "help" | "h" => Command::Help,
        "quit" | "q" | "qa" => Command::Quit,
        "" => return Err(String::new()),
        other => return Err(format!("unknown command :{other} (:help lists them)")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(
            parse("join #general").unwrap(),
            Command::Join("general".into())
        );
        assert_eq!(parse("dm @bob").unwrap(), Command::Dm("bob".into()));
        assert_eq!(
            parse("status idle").unwrap(),
            Command::Status(PresenceStatus::Idle)
        );
        assert_eq!(parse("open 2").unwrap(), Command::Open(2));
        assert!(parse("open 0").is_err());
        assert_eq!(
            parse("instance add hermes.senko.tools").unwrap(),
            Command::AddInstance("hermes.senko.tools".into())
        );
        assert_eq!(
            parse("instance remove").unwrap(),
            Command::RemoveInstance(None)
        );
        assert_eq!(
            parse("set buffer.group_minutes 10").unwrap(),
            Command::Set("buffer.group_minutes".into(), "10".into())
        );
        assert!(parse("set onlykey").is_err());
        assert_eq!(parse("q").unwrap(), Command::Quit);
        assert!(parse("frob").is_err());
        assert!(parse("join").is_err());
    }
}
