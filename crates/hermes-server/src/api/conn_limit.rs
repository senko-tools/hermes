//! Caps on open connections, so that sockets held open on purpose cannot use up the server.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use axum::serve::Listener;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// How many connections may be open.
#[derive(Debug, Clone)]
pub struct Caps {
    pub total: u32,
    /// From one address; 0 = no limit. Trusted proxies carry many people, so they have none.
    pub per_ip: u32,
    pub trusted_proxies: Arc<[IpAddr]>,
}

type Counts = Arc<Mutex<HashMap<IpAddr, u32>>>;

/// A listener that waits while the server is full and turns away one address holding too many.
pub struct Capped<L> {
    inner: L,
    total: Arc<Semaphore>,
    per_ip: u32,
    trusted_proxies: Arc<[IpAddr]>,
    open: Counts,
}

impl<L> Capped<L> {
    pub fn new(inner: L, caps: Caps) -> Self {
        Capped {
            inner,
            total: Arc::new(Semaphore::new(caps.total.max(1) as usize)),
            per_ip: caps.per_ip,
            trusted_proxies: caps.trusted_proxies,
            open: Counts::default(),
        }
    }

    /// One more connection from `ip`, unless it already has all it may.
    fn claim(&self, ip: IpAddr) -> Result<Option<IpSlot>, ()> {
        if self.per_ip == 0 || self.trusted_proxies.contains(&ip) {
            return Ok(None);
        }
        let mut open = self.open.lock().expect("connection counts");
        let count = open.entry(ip).or_insert(0);
        if *count >= self.per_ip {
            return Err(());
        }
        *count += 1;
        Ok(Some(IpSlot {
            ip,
            open: self.open.clone(),
        }))
    }
}

struct IpSlot {
    ip: IpAddr,
    open: Counts,
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut open = self.open.lock().expect("connection counts");
        if let Some(count) = open.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                open.remove(&self.ip);
            }
        }
    }
}

/// A connection that gives its place back when it closes.
pub struct CappedIo<I> {
    io: I,
    _total: OwnedSemaphorePermit,
    _slot: Option<IpSlot>,
}

impl<L> Listener for Capped<L>
where
    L: Listener<Addr = SocketAddr>,
    L::Io: Unpin,
{
    type Io = CappedIo<L::Io>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, SocketAddr) {
        loop {
            let total = self
                .total
                .clone()
                .acquire_owned()
                .await
                .expect("the semaphore is never closed");
            let (io, addr) = self.inner.accept().await;
            if let Ok(slot) = self.claim(addr.ip()) {
                return (
                    CappedIo {
                        io,
                        _total: total,
                        _slot: slot,
                    },
                    addr,
                );
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

impl<I: AsyncRead + Unpin> AsyncRead for CappedIo<I> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl<I: AsyncWrite + Unpin> AsyncWrite for CappedIo<I> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capped(per_ip: u32, trusted: &[IpAddr]) -> Capped<()> {
        Capped::new(
            (),
            Caps {
                total: 10,
                per_ip,
                trusted_proxies: trusted.into(),
            },
        )
    }

    #[test]
    fn one_address_cannot_hold_more_than_its_share() {
        let ip: IpAddr = "203.0.113.5".parse().unwrap();
        let capped = capped(2, &[]);
        let first = capped.claim(ip).unwrap();
        let _second = capped.claim(ip).unwrap();
        assert!(capped.claim(ip).is_err());
        drop(first);
        assert!(capped.claim(ip).is_ok());
    }

    #[test]
    fn a_trusted_proxy_has_no_limit() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let capped = capped(1, &[ip]);
        assert!(capped.claim(ip).unwrap().is_none());
        assert!(capped.claim(ip).unwrap().is_none());
    }
}
