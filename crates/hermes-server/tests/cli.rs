//! Runs the real `hermes-server` binary for `init` and `check-config`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fresh, empty directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("hermes-server-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `hermes-server` in `dir`, without any config-related variables inherited from the caller.
fn server(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hermes-server"));
    cmd.current_dir(dir);
    for (key, _) in std::env::vars_os() {
        let key_str = key.to_string_lossy();
        if key_str.starts_with("HERMES") || key_str == "SERVER_PORT" {
            cmd.env_remove(&key);
        }
    }
    cmd
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn init_writes_template_and_refuses_to_overwrite() {
    let dir = TempDir::new("init");
    let path = dir.path().join("etc").join("hermes.toml");

    let output = server(dir.path())
        .args(["init", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(written, hermes_server::config::DEFAULT_TEMPLATE);

    std::fs::write(&path, "# edited by the admin\n").unwrap();
    let output = server(dir.path())
        .args(["init", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("already exists"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# edited by the admin\n"
    );
}

#[test]
fn check_config_prints_effective_values_of_init_output() {
    let dir = TempDir::new("check");
    // The default `--config` is `./hermes.toml` in the working directory.
    let output = server(dir.path()).arg("init").output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(dir.path().join("hermes.toml").is_file());

    let output = server(dir.path())
        .arg("check-config")
        .env("HERMES__INSTANCE__NAME", "From env")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("[network]"), "{out}");
    assert!(out.contains("port = 7700"), "{out}");
    assert!(out.contains(r#"name = "From env""#), "{out}");
    let err = stderr(&output);
    assert!(err.contains("instance.public_host is empty"), "{err}");
    assert!(err.contains("The configuration is valid"), "{err}");
}

#[test]
fn check_config_without_file_uses_env_and_server_port() {
    let dir = TempDir::new("env-only");
    let output = server(dir.path())
        .arg("check-config")
        .env("SERVER_PORT", "25565")
        .env("HERMES__INSTANCE__PUBLIC_HOST", "hermes.example.org")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("not found"), "{out}");
    assert!(out.contains("port = 25565"), "{out}");
    assert!(!stderr(&output).contains("Warnings"), "{}", stderr(&output));
}

#[test]
fn check_config_uses_hermes_config_env() {
    let dir = TempDir::new("hermes-config-env");
    let path = dir.path().join("custom.toml");
    std::fs::write(&path, "[network]\nport = 7800\n").unwrap();
    let output = server(dir.path())
        .arg("check-config")
        .env("HERMES_CONFIG", &path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("port = 7800"));
}

#[test]
fn check_config_lists_all_errors_and_fails() {
    let dir = TempDir::new("invalid");
    let path = dir.path().join("hermes.toml");
    std::fs::write(&path, "[network]\nport = 0\n[tls]\nmode = \"files\"\n").unwrap();
    let output = server(dir.path()).arg("check-config").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(err.contains("Invalid configuration"), "{err}");
    assert!(err.contains("network.port"), "{err}");
    assert!(err.contains("tls.cert_file"), "{err}");
    assert!(err.contains("tls.key_file"), "{err}");
    assert!(stdout(&output).is_empty());
}

#[test]
fn run_rejects_invalid_config() {
    let dir = TempDir::new("run-invalid");
    std::fs::write(dir.path().join("hermes.toml"), "[network]\nprot = 1\n").unwrap();
    // No subcommand = `run`.
    let output = server(dir.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("prot"), "{}", stderr(&output));
}
