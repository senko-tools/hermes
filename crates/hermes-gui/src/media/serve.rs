//! Hands media in memory to FFmpeg: a tiny HTTP server on the loopback address, one random path
//! per file, with range requests so FFmpeg can seek (an MP4 often keeps its index at the end).
//! Nothing decrypted is written to disk this way.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock, Weak};

struct Server {
    port: u16,
    files: Mutex<HashMap<String, Weak<Vec<u8>>>>,
}

static SERVER: OnceLock<Result<Arc<Server>, String>> = OnceLock::new();

/// A file being served: its address stays valid while this lives.
pub struct Served {
    pub url: String,
    _bytes: Arc<Vec<u8>>,
}

impl Served {
    /// How many bytes the file has.
    pub fn size(&self) -> usize {
        self._bytes.len()
    }
}

/// Serves `bytes` until the returned [`Served`] is dropped.
pub fn serve(bytes: Arc<Vec<u8>>) -> Result<Served, String> {
    let server = SERVER.get_or_init(start).clone()?;
    let path = token();
    let mut files = server.files.lock().unwrap_or_else(|e| e.into_inner());
    // Forget files nobody holds any more.
    files.retain(|_, file| file.strong_count() > 0);
    files.insert(path.clone(), Arc::downgrade(&bytes));
    Ok(Served {
        url: format!("http://127.0.0.1:{}/{path}", server.port),
        _bytes: bytes,
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
                        if let Err(err) = answer(&server, stream) {
                            tracing::debug!("media serve: {err}");
                        }
                    });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(server)
}

/// Answers one request, then closes the connection.
fn answer(server: &Server, stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
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
    let len = file.len() as u64;
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
    let body = if len == 0 {
        &file[..0]
    } else {
        &file[start as usize..=end as usize]
    };
    let status = if range.is_some() {
        format!("206 Partial Content\r\nContent-Range: bytes {start}-{end}/{len}")
    } else {
        "200 OK".to_owned()
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body.len()
    );
    out.write_all(head.as_bytes())?;
    if method != "HEAD" {
        // FFmpeg closes the connection when it seeks: that error is expected.
        let _ = out.write_all(body);
    }
    Ok(())
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
    use std::io::Read as _;

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
        let served = serve(Arc::new(b"hello world".to_vec())).unwrap();
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
