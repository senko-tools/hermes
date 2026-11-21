//! Hands media to FFmpeg: a tiny HTTP server on the loopback address, one random path per file,
//! with range requests so FFmpeg can seek (an MP4 often keeps its index at the end). A file is in
//! memory, or read from the server in parts as FFmpeg asks for them, so it plays while it
//! downloads. Nothing decrypted is written to disk this way.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

/// How long a finished connection waits for the other side to close it.
const CLOSE_WAIT: Duration = Duration::from_secs(1);

struct Server {
    port: u16,
    files: Mutex<HashMap<String, Weak<Source>>>,
}

/// Where a served file's bytes come from.
pub enum Source {
    Memory(Arc<Vec<u8>>),
    /// Read from the server in parts, on the app's async runtime.
    Remote(
        Arc<hermes_client_core::remote_file::RemoteFile>,
        tokio::runtime::Handle,
    ),
}

impl Source {
    fn len(&self) -> u64 {
        match self {
            Source::Memory(bytes) => bytes.len() as u64,
            Source::Remote(file, _) => file.size(),
        }
    }
}

/// The first part fetched for a request, and the most: each part fetched is twice the last, so
/// playback starts after a small one and then reads in few requests.
const FIRST_PART: u64 = 256 * 1024;
const MAX_PART: u64 = 4 * 1024 * 1024;

static SERVER: OnceLock<Result<Arc<Server>, String>> = OnceLock::new();

/// A file being served: its address stays valid while this lives.
pub struct Served {
    pub url: String,
    source: Arc<Source>,
}

impl Served {
    /// How many of its bytes are in memory.
    pub fn size(&self) -> usize {
        match &*self.source {
            Source::Memory(bytes) => bytes.len(),
            Source::Remote(file, _) => file.kept_bytes(),
        }
    }
}

/// Serves `source` until the returned [`Served`] is dropped.
pub fn serve(source: Source) -> Result<Served, String> {
    let server = SERVER.get_or_init(start).clone()?;
    let path = token();
    let source = Arc::new(source);
    let mut files = server.files.lock().unwrap_or_else(|e| e.into_inner());
    // Forget files nobody holds any more.
    files.retain(|_, file| file.strong_count() > 0);
    files.insert(path.clone(), Arc::downgrade(&source));
    Ok(Served {
        url: format!("http://127.0.0.1:{}/{path}", server.port),
        source,
    })
}

/// 128 random bits, from the OS's hasher seeds (no new dependency).
fn token() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut out = String::new();
    for n in 0..2u64 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(n);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out
}

fn start() -> Result<Arc<Server>, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let server = Arc::new(Server {
        port,
        files: Mutex::new(HashMap::new()),
    });
    let shared = server.clone();
    std::thread::Builder::new()
        .name("media-serve".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let server = shared.clone();
                let _ = std::thread::Builder::new()
                    .name("media-serve-conn".into())
                    .spawn(move || {
                        if let Err(err) = answer(&server, &stream) {
                            tracing::debug!("media serve: {err}");
                        }
                        close(&stream);
                    });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(server)
}

/// Ends a connection in order: says it is done sending, then waits for the other side to finish.
/// Just dropping the socket can reset it on Windows before the reader has got the last bytes.
fn close(mut stream: &TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(CLOSE_WAIT));
    let mut rest = [0u8; 512];
    while matches!(stream.read(&mut rest), Ok(n) if n > 0) {}
}

/// Answers one request.
fn answer(server: &Server, stream: &TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut range = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_owned());
        }
    }
    let file = server
        .files
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(target.trim_start_matches('/'))
        .and_then(Weak::upgrade);
    let mut out = stream;
    let Some(file) = file else {
        return out.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    };
    let len = file.len();
    let (start, end) = match range.as_deref().and_then(|r| parse_range(r, len)) {
        Some(span) => span,
        None if range.is_some() => {
            let head = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            return out.write_all(head.as_bytes());
        }
        None => (0, len.saturating_sub(1)),
    };
    let body_len = if len == 0 { 0 } else { end - start + 1 };
    let status = if range.is_some() {
        format!("206 Partial Content\r\nContent-Range: bytes {start}-{end}/{len}")
    } else {
        "200 OK".to_owned()
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body_len
    );
    out.write_all(head.as_bytes())?;
    if method == "HEAD" || body_len == 0 {
        return Ok(());
    }
    // FFmpeg closes the connection when it seeks: a write error then is expected.
    match &*file {
        Source::Memory(bytes) => {
            let _ = out.write_all(&bytes[start as usize..=end as usize]);
        }
        Source::Remote(remote, runtime) => {
            stream_remote(remote, runtime, start, end + 1, out);
        }
    }
    Ok(())
}

/// Writes `from..to` of a remote file as its parts arrive, fetching the next part while the
/// last is written.
fn stream_remote(
    file: &Arc<hermes_client_core::remote_file::RemoteFile>,
    runtime: &tokio::runtime::Handle,
    from: u64,
    to: u64,
    mut out: &TcpStream,
) {
    let fetch = |at: u64, len: u64| {
        let file = file.clone();
        runtime.spawn(async move { file.read(at, len).await })
    };
    let mut part = FIRST_PART;
    let mut at = from;
    let mut next = Some(fetch(at, part.min(to - at)));
    while let Some(pending) = next.take() {
        let bytes = match runtime.block_on(pending) {
            Ok(Ok(bytes)) if !bytes.is_empty() => bytes,
            Ok(Err(err)) => {
                tracing::warn!("media part at {at}: {err}");
                return;
            }
            _ => return,
        };
        at += bytes.len() as u64;
        if at < to {
            part = (part * 2).min(MAX_PART);
            next = Some(fetch(at, part.min(to - at)));
        }
        if out.write_all(&bytes).is_err() {
            // FFmpeg went elsewhere: what was being fetched is kept for later.
            return;
        }
    }
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` as an inclusive span within `len`.
fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (len.saturating_sub(n), len - 1)
        }
        (a, "") => (a.parse().ok()?, len - 1),
        (a, b) => (a.parse().ok()?, b.parse::<u64>().ok()?.min(len - 1)),
    };
    (start <= end && start < len).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=50-500", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
    }

    fn get(url: &str, range: Option<&str>) -> String {
        let rest = url.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').unwrap();
        let mut stream = TcpStream::connect(host).unwrap();
        let range = range.map_or(String::new(), |r| format!("Range: {r}\r\n"));
        write!(stream, "GET /{path} HTTP/1.1\r\nHost: x\r\n{range}\r\n").unwrap();
        let mut out = String::new();
        stream.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serves_a_file_and_its_ranges_until_dropped() {
        let served = serve(Source::Memory(Arc::new(b"hello world".to_vec()))).unwrap();
        let whole = get(&served.url, None);
        assert!(whole.starts_with("HTTP/1.1 200"), "{whole}");
        assert!(whole.ends_with("hello world"));
        let part = get(&served.url, Some("bytes=6-"));
        assert!(part.contains("Content-Range: bytes 6-10/11"), "{part}");
        assert!(part.ends_with("world"));
        let url = served.url.clone();
        drop(served);
        assert!(get(&url, None).starts_with("HTTP/1.1 404"));
    }
}
