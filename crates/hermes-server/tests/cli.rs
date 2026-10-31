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

#[test]
fn invite_create_prints_links() {
    let dir = TempDir::new("invite-create");
    let with_env = |cmd: &mut Command| {
        cmd.env("HERMES__INSTANCE__PUBLIC_HOST", "hermes.example.org")
            .env("HERMES__NETWORK__PORT", "7800");
    };

    // Self-signed (the default): the links carry the certificate fingerprint.
    let mut cmd = server(dir.path());
    with_env(&mut cmd);
    let output = cmd
        .args(["invite", "create", "--uses", "3", "--expires", "24h"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("joins no space"), "{out}");
    assert!(out.contains("Uses: 3"), "{out}");
    assert!(out.contains("Expires: in 1d"), "{out}");
    assert!(
        out.contains("https://hermes.example.org:7800/invite/"),
        "{out}"
    );
    assert!(
        out.contains("hermes://hermes.example.org:7800/invite/"),
        "{out}"
    );
    assert!(out.contains("#fp=sha256:"), "{out}");
    assert!(dir.path().join("data").join("hermes.db").is_file());

    // TLS off: no fingerprint; unknown spaces and bad lifetimes are rejected.
    let mut cmd = server(dir.path());
    with_env(&mut cmd);
    let output = cmd
        .env("HERMES__TLS__MODE", "off")
        .args(["invite", "create", "--expires", "never"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("Uses: unlimited") && out.contains("Expires: never"),
        "{out}"
    );
    assert!(!out.contains("#fp="), "{out}");

    let output = server(dir.path())
        .args(["invite", "create", "--space", "12345"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("no space with ID 12345"),
        "{}",
        stderr(&output)
    );

    let output = server(dir.path())
        .args(["invite", "create", "--expires", "soon"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("not a lifetime"),
        "{}",
        stderr(&output)
    );
}

/// A free local TCP port (freed again before returning).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `GET path` over plain HTTP/1.1; returns the whole response.
fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

/// Lines of a child's stdout, read on a thread so waiting can time out.
struct Lines(std::sync::mpsc::Receiver<String>);

impl Lines {
    fn new(child: &mut std::process::Child) -> Self {
        use std::io::{BufRead, BufReader};
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self(rx)
    }

    /// Waits (at most 30 s) for a line containing `needle`.
    #[track_caller]
    fn wait_for(&self, needle: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.0.recv_timeout(left) {
                Ok(line) if line.contains(needle) => return,
                Ok(line) => seen.push(line),
                Err(_) => panic!("no line with {needle:?}; got: {seen:#?}"),
            }
        }
    }
}

/// `hermes-server run` on `127.0.0.1:<port>` with JSON logs and stdout piped.
fn run(dir: &Path, port: u16, env: &[(&str, &str)]) -> (std::process::Child, Lines) {
    let mut cmd = server(dir);
    cmd.arg("run")
        .env("HERMES__NETWORK__BIND", "127.0.0.1")
        .env("HERMES__NETWORK__PORT", port.to_string())
        .env("HERMES__LOG__FORMAT", "json")
        .stdout(std::process::Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }
    let mut child = cmd.spawn().unwrap();
    let lines = Lines::new(&mut child);
    (child, lines)
}

#[cfg(unix)]
fn signal(child: &std::process::Child, name: &str) {
    let status = Command::new("kill")
        .args([&format!("-{name}"), &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn run_logs_ready_line_and_serves_healthz() {
    let dir = TempDir::new("run");
    let port = free_port();
    let (mut child, lines) = run(dir.path(), port, &[("HERMES__TLS__MODE", "off")]);
    lines.wait_for(&format!("Hermes is ready on 127.0.0.1:{port}"));
    let response = http_get(port, "/healthz");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("ok"), "{response}");

    #[cfg(unix)]
    {
        signal(&child, "TERM");
        lines.wait_for("Hermes stopped");
        assert!(child.wait().unwrap().success());
    }
    #[cfg(not(unix))]
    {
        // Sending Ctrl+C to one process is not practical on Windows; the shutdown path itself
        // is covered by `lifecycle::shutdown_closes_gateways_with_4100_and_stops_serving`.
        child.kill().unwrap();
        child.wait().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn sighup_reloads_the_certificate() {
    let dir = TempDir::new("sighup");
    let first =
        hermes_server::tls::load_or_generate(dir.path(), "a", &["localhost".to_owned()]).unwrap();
    let second =
        hermes_server::tls::load_or_generate(dir.path(), "b", &["localhost".to_owned()]).unwrap();
    let (cert, key) = (
        dir.path().join("live-cert.pem"),
        dir.path().join("live-key.pem"),
    );
    std::fs::copy(&first.cert_path, &cert).unwrap();
    std::fs::copy(&first.key_path, &key).unwrap();
    let port = free_port();
    let (mut child, lines) = run(
        dir.path(),
        port,
        &[
            ("HERMES__TLS__MODE", "files"),
            ("HERMES__TLS__CERT_FILE", cert.to_str().unwrap()),
            ("HERMES__TLS__KEY_FILE", key.to_str().unwrap()),
        ],
    );
    lines.wait_for("Hermes is ready on");

    std::fs::copy(&second.cert_path, &cert).unwrap();
    std::fs::copy(&second.key_path, &key).unwrap();
    signal(&child, "HUP");
    let fingerprint = hermes_server::tls::Fingerprint::of(&second.chain[0]).to_string();
    lines.wait_for(&fingerprint);

    // Still running after a reload with a broken file; the old certificate stays.
    std::fs::write(&key, "garbage").unwrap();
    signal(&child, "HUP");
    lines.wait_for("still using the old one");
    signal(&child, "TERM");
    lines.wait_for("Hermes stopped");
    assert!(child.wait().unwrap().success());
}
