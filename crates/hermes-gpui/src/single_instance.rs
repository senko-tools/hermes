//! Single instance (PLAN.md section 15.7): a second launch (e.g. from a `hermes://` link) hands
//! its argument to the running client and exits. One channel per config directory: a named pipe
//! on Windows, a unix socket in the config directory elsewhere. A message is one line: the link,
//! or nothing ("show yourself").

use std::io;
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// Longest accepted message.
const MAX_LINE: u64 = 4096;

/// Where the running client listens.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint(#[cfg(windows)] String, #[cfg(not(windows))] PathBuf);

impl Endpoint {
    pub fn for_config_dir(dir: &Path) -> Endpoint {
        #[cfg(windows)]
        {
            // Pipe names cannot contain backslashes; a hash keeps them short and unique.
            let dir = dir.to_string_lossy().to_lowercase();
            Endpoint(format!(r"\\.\pipe\hermes-{:016x}", fnv1a(dir.as_bytes())))
        }
        #[cfg(not(windows))]
        {
            Endpoint(dir.join("hermes.sock"))
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Sends `link` (or an empty line) to a running client. An error means none is running.
pub async fn forward(endpoint: &Endpoint, link: Option<&str>) -> io::Result<()> {
    let line = format!("{}\n", link.unwrap_or("").replace(['\r', '\n'], ""));
    tokio::time::timeout(Duration::from_secs(2), async {
        #[cfg(windows)]
        let mut stream = tokio::net::windows::named_pipe::ClientOptions::new().open(&endpoint.0)?;
        #[cfg(not(windows))]
        let mut stream = tokio::net::UnixStream::connect(&endpoint.0).await?;
        stream.write_all(line.as_bytes()).await?;
        stream.flush().await?;
        stream.shutdown().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no answer"))?
}

/// Accepts forwarded messages until `tx` is closed. Fails if another client already listens.
pub async fn serve(endpoint: Endpoint, tx: mpsc::UnboundedSender<String>) -> io::Result<()> {
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ServerOptions;

        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&endpoint.0)?;
        loop {
            server.connect().await?;
            let connected = server;
            // The next instance exists before this one is handled, so no launch is missed.
            server = ServerOptions::new().create(&endpoint.0)?;
            if !read_one(connected, &tx).await {
                return Ok(());
            }
        }
    }
    #[cfg(not(windows))]
    {
        let path = &endpoint.0;
        let listener = match tokio::net::UnixListener::bind(path) {
            Ok(listener) => listener,
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
                if tokio::net::UnixStream::connect(path).await.is_ok() {
                    return Err(err);
                }
                // Left over from a client that crashed.
                let _ = std::fs::remove_file(path);
                tokio::net::UnixListener::bind(path)?
            }
            Err(err) => return Err(err),
        };
        let _cleanup = RemoveOnDrop(path.clone());
        loop {
            let (stream, _) = listener.accept().await?;
            if !read_one(stream, &tx).await {
                return Ok(());
            }
        }
    }
}

/// Reads one line; `false` when nobody listens any more.
async fn read_one(
    stream: impl tokio::io::AsyncRead + Unpin,
    tx: &mpsc::UnboundedSender<String>,
) -> bool {
    let mut line = String::new();
    let mut reader = BufReader::new(stream.take(MAX_LINE));
    let read = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line)).await;
    match read {
        // `0`: closed without a line (a probe from `serve`), not a request.
        Ok(Ok(n)) if n > 0 => tx.send(line.trim().to_owned()).is_ok(),
        _ => !tx.is_closed(),
    }
}

#[cfg(not(windows))]
struct RemoveOnDrop(PathBuf);

#[cfg(not(windows))]
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn second_launch_forwards_to_the_first() {
        let dir = std::env::temp_dir().join(format!("hermes-gui-{}-single", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let endpoint = Endpoint::for_config_dir(&dir);

        assert!(forward(&endpoint, None).await.is_err(), "nothing runs yet");

        let (tx, mut rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(serve(endpoint.clone(), tx));
        // Wait until it listens.
        let mut sent = false;
        for _ in 0..50 {
            if forward(&endpoint, Some("hermes://h/invite/abc#fp=x"))
                .await
                .is_ok()
            {
                sent = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(sent);
        assert_eq!(rx.recv().await.unwrap(), "hermes://h/invite/abc#fp=x");

        forward(&endpoint, None).await.unwrap();
        assert_eq!(rx.recv().await.unwrap(), "");

        // A second listener for the same directory is refused.
        let (tx2, _rx2) = mpsc::unbounded_channel();
        assert!(serve(endpoint.clone(), tx2).await.is_err());

        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn endpoints_differ_per_directory() {
        let a = Endpoint::for_config_dir(Path::new("/a/hermes"));
        let b = Endpoint::for_config_dir(Path::new("/b/hermes"));
        assert_ne!(a, b);
    }
}
