//! `hermes-server user …` (PLAN.md section 9.1): account administration straight on the
//! database. Safe while the server runs; it applies the changes within 10 seconds.

use std::path::Path;
use std::process::ExitCode;

use clap::Subcommand;
use hermes_proto::id::Id;

use super::{load_or_report, small_runtime};
use crate::db::{self, Db};

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// List every account.
    List,
    /// Set a new random password, sign the user out everywhere and print the password.
    ResetPassword { username: String },
    /// Give (`true`) or take (`false`) instance admin rights.
    SetAdmin {
        username: String,
        #[arg(action = clap::ArgAction::Set)]
        admin: bool,
    },
    /// Disable an account and sign it out everywhere.
    Disable { username: String },
    /// Enable a disabled account again.
    Enable { username: String },
    /// Let an account go without a `#1234` tag (`true`), or give it one again (`false`).
    SetTagless {
        username: String,
        #[arg(action = clap::ArgAction::Set)]
        tagless: bool,
    },
}

/// A random password of 16 letters and digits (no look-alikes such as 0/O and 1/l).
pub fn random_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

pub fn run(path: &Path, command: UserCommand) -> ExitCode {
    let Some(loaded) = load_or_report(path) else {
        return ExitCode::FAILURE;
    };
    let Some(runtime) = small_runtime() else {
        return ExitCode::FAILURE;
    };
    let data_dir = loaded.config.storage.data_dir.clone();
    runtime.block_on(async move {
        let db = match Db::open(&data_dir).await {
            Ok(db) => db,
            Err(err) => {
                eprintln!(
                    "Could not open the database in {}: {err}",
                    data_dir.display()
                );
                return ExitCode::FAILURE;
            }
        };
        let code = match execute(&db, command).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        };
        db.close().await;
        code
    })
}

async fn find(db: &Db, username: &str) -> Result<(Id, String, bool, bool), String> {
    let login = username.trim_start_matches('@').to_lowercase();
    let handle = db::auth::resolve_handle(&db.read, &login)
        .await
        .map_err(|e| format!("Database error: {e}"))?
        .map_err(|()| format!("Several accounts are called {username:?}: add the # number"))?
        .ok_or_else(|| format!("There is no user called {username:?}"))?;
    db::users::by_username(&db.read, &handle)
        .await
        .map_err(|e| format!("Database error: {e}"))?
        .ok_or_else(|| format!("There is no user called {username:?}"))
}

async fn execute(db: &Db, command: UserCommand) -> Result<(), String> {
    let dberr = |e: sqlx::Error| format!("Database error: {e}");
    match command {
        UserCommand::List => {
            let users = db::users::list(&db.read).await.map_err(dberr)?;
            println!("{:<20} {:<24} {:<10} created", "ID", "username", "flags");
            for (id, username, display, disabled, admin, created_at) in users {
                let mut flags = Vec::new();
                if admin {
                    flags.push("admin");
                }
                if disabled {
                    flags.push("disabled");
                }
                let name = match display {
                    Some(d) => format!("{username} ({d})"),
                    None => username,
                };
                println!(
                    "{:<20} {:<24} {:<10} {}",
                    id.to_string(),
                    name,
                    flags.join(","),
                    crate::time::format_date(created_at)
                );
            }
        }
        UserCommand::SetTagless { username, tagless } => {
            let (id, handle, ..) = find(db, &username).await?;
            let (name, tag) = hermes_proto::models::split_handle(&handle);
            let new = if tagless == tag.is_none() {
                handle.clone()
            } else if tagless {
                name.to_owned()
            } else {
                let start = rand::random_range(0..9999u16);
                let mut free = None;
                for i in 0..9999u16 {
                    let candidate = format!("{name}#{:04}", (start + i) % 9999 + 1);
                    if db::users::by_username(&db.read, &candidate)
                        .await
                        .map_err(dberr)?
                        .is_none()
                    {
                        free = Some(candidate);
                        break;
                    }
                }
                free.ok_or_else(|| format!("All tags of {name:?} are taken"))?
            };
            if new != handle {
                if tagless
                    && db::users::by_username(&db.read, &new)
                        .await
                        .map_err(dberr)?
                        .is_some()
                {
                    return Err(format!("Another account is already called {new:?}"));
                }
                let mut conn = db.write.acquire().await.map_err(dberr)?;
                db::users::set_handle(&mut conn, id, &new)
                    .await
                    .map_err(dberr)?;
            }
            println!("{handle} is now {new}.");
        }
        UserCommand::ResetPassword { username } => {
            let (id, name, ..) = find(db, &username).await?;
            let password = random_password();
            let hash = crate::auth::hash_password(password.clone())
                .await
                .map_err(|e| format!("Could not hash the password: {}", e.0.message))?;
            let mut tx = db.write.begin().await.map_err(dberr)?;
            db::auth::set_password_hash(&mut tx, id, &hash)
                .await
                .map_err(dberr)?;
            let revoked = db::auth::delete_other_sessions(&mut tx, id, Id(0))
                .await
                .map_err(dberr)?;
            tx.commit().await.map_err(dberr)?;
            println!("New password for {name}: {password}");
            println!(
                "{} session(s) signed out. Ask them to change it in Settings → Account.",
                revoked.len()
            );
        }
        UserCommand::SetAdmin { username, admin } => {
            let (id, name, disabled, _) = find(db, &username).await?;
            let mut tx = db.write.begin().await.map_err(dberr)?;
            db::users::set_flags(&mut tx, id, disabled, admin)
                .await
                .map_err(dberr)?;
            tx.commit().await.map_err(dberr)?;
            println!(
                "{name} is {} an instance admin",
                if admin { "now" } else { "no longer" }
            );
        }
        UserCommand::Disable { username } => {
            let (id, name, _, admin) = find(db, &username).await?;
            let mut tx = db.write.begin().await.map_err(dberr)?;
            db::users::set_flags(&mut tx, id, true, admin)
                .await
                .map_err(dberr)?;
            let revoked = db::auth::delete_other_sessions(&mut tx, id, Id(0))
                .await
                .map_err(dberr)?;
            tx.commit().await.map_err(dberr)?;
            println!(
                "{name} is disabled; {} session(s) signed out",
                revoked.len()
            );
        }
        UserCommand::Enable { username } => {
            let (id, name, _, admin) = find(db, &username).await?;
            let mut tx = db.write.begin().await.map_err(dberr)?;
            db::users::set_flags(&mut tx, id, false, admin)
                .await
                .map_err(dberr)?;
            tx.commit().await.map_err(dberr)?;
            println!("{name} can sign in again");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_long_and_unambiguous() {
        let p = random_password();
        assert_eq!(p.len(), 16);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(!p.contains(['0', 'O', '1', 'l', 'I']));
        assert_ne!(random_password(), p);
    }
}
