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
    /// Show the last status messages.
    Messages,
    Quit,
    /// `:search <words>` in the open channel's space (everywhere from a DM).
    Search(String),
    /// `:search! <words>`: everywhere on the instance.
    SearchAll(String),
    Mentions,
    Pins,
    /// `:notify all|mentions|none|default|mute|unmute [space]`: the open channel, or its space.
    Notify(Notify, bool),
    /// `:create text|voice|category <name>` in the open channel's space (in its category).
    Create(CreateKind, String),
    /// `:space <name>`: create a space.
    CreateSpace(String),
    Rename(String),
    /// `:topic [text]`; empty clears it.
    Topic(String),
    /// `:delete-channel`: delete the open channel (asks first).
    DeleteChannel,
    Kick(String),
    /// `:ban <user> [reason]`.
    Ban(String, String),
    Unban(String),
    /// `:role <user> +name|-name`.
    Role(String, String, bool),
    /// `:profile name <text>` / `:profile status <text>` (empty clears).
    Profile(ProfileField, String),
    /// `:read`: mark the open channel's space (or the DM) read.
    MarkRead,
    /// `:menu`: the actions of the selection (like `a` or a right-click).
    Menu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notify {
    All,
    Mentions,
    Nothing,
    Default,
    Mute,
    Unmute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateKind {
    Text,
    Voice,
    Category,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileField {
    Name,
    Status,
}

pub const HELP: &str = ":join <channel>  :dm <user>  :voice [channel]  :leave  :mute  :deafen  :status <s>  :upload <path>  :open <n>  \
:invite  :instance add <link|host>  :instance remove  :login  :logout  :set <key> <value>  \
:offline  :online  :retry  :messages  :quit  \
:search <words> (:search! everywhere)  :mentions  :pins  :read  :menu  \
:notify all|mentions|none|default|mute|unmute [space]  \
:create text|voice|category <name>  :space <name>  :rename <name>  :topic [text]  :delete-channel  \
:kick <user>  :ban <user> [reason]  :unban <user>  :role <user> +role|-role  \
:profile name|status <text>";

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
        "search" | "s" => Command::Search(need(":search <words>")?),
        "search!" | "s!" => Command::SearchAll(need(":search! <words>")?),
        "mentions" => Command::Mentions,
        "pins" => Command::Pins,
        "read" => Command::MarkRead,
        "menu" => Command::Menu,
        "notify" => {
            let usage = ":notify all|mentions|none|default|mute|unmute [space]";
            let (what, scope) = match arg.split_once(char::is_whitespace) {
                Some((w, s)) => (w, s.trim()),
                None => (arg, ""),
            };
            let level = match what {
                "all" => Notify::All,
                "mentions" => Notify::Mentions,
                "none" | "nothing" => Notify::Nothing,
                "default" => Notify::Default,
                "mute" => Notify::Mute,
                "unmute" => Notify::Unmute,
                _ => return Err(format!("usage: {usage}")),
            };
            match scope {
                "" => Command::Notify(level, false),
                "space" => Command::Notify(level, true),
                _ => return Err(format!("usage: {usage}")),
            }
        }
        "create" => {
            let usage = ":create text|voice|category <name>";
            let rest = need(usage)?;
            let (kind, name) = rest
                .split_once(char::is_whitespace)
                .ok_or_else(|| format!("usage: {usage}"))?;
            let kind = match kind {
                "text" | "channel" => CreateKind::Text,
                "voice" => CreateKind::Voice,
                "category" => CreateKind::Category,
                _ => return Err(format!("usage: {usage}")),
            };
            Command::Create(kind, name.trim().to_owned())
        }
        "space" => Command::CreateSpace(need(":space <name>")?),
        "rename" => Command::Rename(need(":rename <name>")?),
        "topic" => Command::Topic(arg.to_owned()),
        "delete-channel" => Command::DeleteChannel,
        "kick" => Command::Kick(need(":kick <user>")?.trim_start_matches('@').to_owned()),
        "ban" => {
            let rest = need(":ban <user> [reason]")?;
            let (user, reason) = match rest.split_once(char::is_whitespace) {
                Some((u, r)) => (u, r.trim()),
                None => (rest.as_str(), ""),
            };
            Command::Ban(user.trim_start_matches('@').to_owned(), reason.to_owned())
        }
        "unban" => Command::Unban(need(":unban <user>")?.trim_start_matches('@').to_owned()),
        "role" => {
            let usage = ":role <user> +role|-role";
            let rest = need(usage)?;
            let (user, role) = rest
                .split_once(char::is_whitespace)
                .ok_or_else(|| format!("usage: {usage}"))?;
            let role = role.trim();
            let (add, name) = match role.chars().next() {
                Some('+') => (true, &role[1..]),
                Some('-') => (false, &role[1..]),
                _ => return Err(format!("usage: {usage}")),
            };
            if name.trim().is_empty() {
                return Err(format!("usage: {usage}"));
            }
            Command::Role(
                user.trim_start_matches('@').to_owned(),
                name.trim().to_owned(),
                add,
            )
        }
        "profile" => {
            let usage = ":profile name|status <text>";
            let (field, text) = match arg.split_once(char::is_whitespace) {
                Some((f, t)) => (f, t.trim()),
                None => (arg, ""),
            };
            let field = match field {
                "name" => ProfileField::Name,
                "status" => ProfileField::Status,
                _ => return Err(format!("usage: {usage}")),
            };
            Command::Profile(field, text.to_owned())
        }
        "help" | "h" => Command::Help,
        "messages" | "log" => Command::Messages,
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
        assert_eq!(
            parse("search pizza party").unwrap(),
            Command::Search("pizza party".into())
        );
        assert_eq!(
            parse("notify mute space").unwrap(),
            Command::Notify(Notify::Mute, true)
        );
        assert!(parse("notify loud").is_err());
        assert_eq!(
            parse("create voice Game Night").unwrap(),
            Command::Create(CreateKind::Voice, "Game Night".into())
        );
        assert_eq!(
            parse("ban @bob spam links").unwrap(),
            Command::Ban("bob".into(), "spam links".into())
        );
        assert_eq!(
            parse("role alice +mods").unwrap(),
            Command::Role("alice".into(), "mods".into(), true)
        );
        assert!(parse("role alice mods").is_err());
        assert_eq!(
            parse("profile status").unwrap(),
            Command::Profile(ProfileField::Status, String::new())
        );
        assert_eq!(parse("topic").unwrap(), Command::Topic(String::new()));
    }
}
