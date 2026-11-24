//! Screen and window capture on X11, with x11rb (no Xlib).
//!
//! - Screens: RandR monitors, captured from the root window with MIT-SHM: the X server writes
//!   the picture into shared memory, so a 1080p frame costs one copy instead of 8 MB through the
//!   socket. Without MIT-SHM 1.2 (a remote X server) the picture comes through the socket.
//! - Windows: the EWMH client list (`_NET_CLIENT_LIST`). With a compositing window manager the
//!   window's frame has an off-screen picture of its own (Composite), so covered parts show too,
//!   like Windows' PrintWindow; without one, what is visible of the window on the screen.
//!
//! Pixels come as BGRX (the alpha byte is undefined), 4 bytes each, no padding.

use std::fs::File;

use x11rb::connection::Connection as _;
use x11rb::protocol::composite::ConnectionExt as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

use super::codec::Frame;

/// A RandR monitor (one screen as people see it).
#[derive(Debug, Clone)]
pub struct Monitor {
    pub name: String,
    pub primary: bool,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

/// A window that could be shared.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    /// The app, from `WM_CLASS` (`firefox`, `org.gnome.Nautilus`).
    pub app: String,
    pub pid: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub minimized: bool,
}

fn connect() -> Result<(RustConnection, Window), String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X display: {e}"))?;
    let root = conn.setup().roots[screen].root;
    Ok((conn, root))
}

fn atom(conn: &RustConnection, name: &str) -> Result<u32, String> {
    Ok(conn
        .intern_atom(false, name.as_bytes())
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom)
}

/// The monitors, in the server's order.
pub fn monitors() -> Result<Vec<Monitor>, String> {
    let (conn, root) = connect()?;
    let reply = conn
        .randr_get_monitors(root, true)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("no RandR monitors: {e}"))?;
    let mut out = Vec::new();
    for m in reply.monitors {
        let name = conn
            .get_atom_name(m.name)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.name).into_owned())
            .unwrap_or_default();
        out.push(Monitor {
            name,
            primary: m.primary,
            x: m.x,
            y: m.y,
            width: m.width,
            height: m.height,
        });
    }
    // An X server without monitors (Xvfb with old RandR): the whole root window.
    if out.is_empty() {
        let geometry = conn
            .get_geometry(root)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        out.push(Monitor {
            name: "screen".into(),
            primary: true,
            x: 0,
            y: 0,
            width: geometry.width,
            height: geometry.height,
        });
    }
    Ok(out)
}

/// The top-level windows the window manager lists, topmost first.
pub fn windows() -> Result<Vec<WindowInfo>, String> {
    let (conn, root) = connect()?;
    let list = |name: &str| -> Result<Vec<u32>, String> {
        let atom = atom(&conn, name)?;
        let reply = conn
            .get_property(false, root, atom, AtomEnum::WINDOW, 0, u32::MAX)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(reply.value32().map(Iterator::collect).unwrap_or_default())
    };
    // Bottom to top: reversed, the topmost comes first.
    let mut ids = list("_NET_CLIENT_LIST_STACKING")?;
    if ids.is_empty() {
        ids = list("_NET_CLIENT_LIST")?;
    }
    ids.reverse();
    let net_name = atom(&conn, "_NET_WM_NAME")?;
    let utf8 = atom(&conn, "UTF8_STRING")?;
    let net_pid = atom(&conn, "_NET_WM_PID")?;
    let net_state = atom(&conn, "_NET_WM_STATE")?;
    let hidden = atom(&conn, "_NET_WM_STATE_HIDDEN")?;
    let mut out = Vec::new();
    for id in ids {
        let property = |prop: u32, kind: u32| {
            conn.get_property(false, id, prop, kind, 0, 4096)
                .ok()
                .and_then(|c| c.reply().ok())
        };
        let title = property(net_name, utf8)
            .filter(|r| !r.value.is_empty())
            .or_else(|| property(AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()))
            .map(|r| String::from_utf8_lossy(&r.value).into_owned())
            .unwrap_or_default();
        // WM_CLASS is "instance\0class\0": the class names the app.
        let app = property(AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())
            .map(|r| {
                r.value
                    .split(|b| *b == 0)
                    .rfind(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let pid = property(net_pid, AtomEnum::CARDINAL.into())
            .and_then(|r| r.value32().and_then(|mut v| v.next()));
        let minimized = property(net_state, AtomEnum::ATOM.into())
            .and_then(|r| r.value32().map(|mut v| v.any(|a| a == hidden)))
            .unwrap_or(false);
        let Some(geometry) = conn.get_geometry(id).ok().and_then(|c| c.reply().ok()) else {
            continue;
        };
        out.push(WindowInfo {
            id,
            title,
            app,
            pid,
            width: u32::from(geometry.width),
            height: u32::from(geometry.height),
            minimized,
        });
    }
    Ok(out)
}

/// Why a picture could not be taken.
#[derive(Debug)]
pub enum GrabError {
    /// Minimized (or unmapped for now): try again later.
    Hidden,
    /// The window or the display is gone.
    Gone(String),
}

/// What a `Grabber` takes pictures of.
enum Target {
    Area {
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    },
    Window(Window),
}

/// Shared memory the X server writes pictures into.
struct Segment {
    seg: shm::Seg,
    map: memmap2::Mmap,
}

/// Takes pictures of one screen or window, again and again.
pub struct Grabber {
    conn: RustConnection,
    root: Window,
    target: Target,
    segment: Option<Segment>,
    /// MIT-SHM 1.2 (segments made by the server) is there.
    shm: bool,
    /// Windows can be captured off-screen (a compositing window manager is running).
    composite: bool,
    net_state: u32,
    hidden: u32,
    /// XFixes can say what the pointer looks like (X11 pictures leave it out).
    cursor: bool,
}

impl Grabber {
    pub fn screen(monitor: &Monitor) -> Result<Grabber, String> {
        Grabber::new(Target::Area {
            x: monitor.x,
            y: monitor.y,
            width: monitor.width,
            height: monitor.height,
        })
    }

    pub fn window(id: u32) -> Result<Grabber, String> {
        Grabber::new(Target::Window(id))
    }

    fn new(target: Target) -> Result<Grabber, String> {
        let (conn, root) = connect()?;
        let shm = conn
            .shm_query_version()
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|v| (v.major_version, v.minor_version) >= (1, 2));
        // Composite is useful only while a compositing manager redirects the windows: the
        // selection `_NET_WM_CM_S<screen>` has an owner then.
        let composite = conn
            .composite_query_version(0, 4)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some()
            && atom(&conn, "_NET_WM_CM_S0")
                .ok()
                .and_then(|a| conn.get_selection_owner(a).ok())
                .and_then(|c| c.reply().ok())
                .is_some_and(|r| r.owner != x11rb::NONE);
        let net_state = atom(&conn, "_NET_WM_STATE")?;
        let hidden = atom(&conn, "_NET_WM_STATE_HIDDEN")?;
        // The version must be asked for before XFixes' requests may be used.
        let cursor = conn
            .xfixes_query_version(4, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|v| v.major_version >= 1);
        Ok(Grabber {
            conn,
            root,
            target,
            segment: None,
            shm,
            composite,
            net_state,
            hidden,
            cursor,
        })
    }

    /// "MIT-SHM" or "XGetImage", and for windows whether covered parts show, for the logs.
    pub fn describe(&self) -> String {
        let how = if self.shm { "X11 SHM" } else { "X11 GetImage" };
        match self.target {
            Target::Area { .. } => how.to_owned(),
            Target::Window(_) if self.composite => format!("{how}, composited window"),
            Target::Window(_) => format!("{how}, visible part of the window"),
        }
    }

    /// One picture, BGRX, with the pointer drawn in (as Windows' capture has it).
    pub fn grab(&mut self) -> Result<Frame, GrabError> {
        let mut frame = self.grab_bare()?;
        if self.cursor {
            let origin = match self.target {
                Target::Area { x, y, .. } => Some((i32::from(x), i32::from(y))),
                Target::Window(id) => self
                    .conn
                    .translate_coordinates(id, self.root, 0, 0)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| (i32::from(r.dst_x), i32::from(r.dst_y))),
            };
            if let Some(origin) = origin {
                self.draw_cursor(&mut frame, origin);
            }
        }
        Ok(frame)
    }

    /// Blends the pointer (premultiplied ARGB) over `frame`, whose top left is at `origin` on
    /// the root window.
    fn draw_cursor(&self, frame: &mut Frame, (ox, oy): (i32, i32)) {
        let Some(image) = self
            .conn
            .xfixes_get_cursor_image()
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            return;
        };
        let (cw, ch) = (i32::from(image.width), i32::from(image.height));
        let left = i32::from(image.x) - i32::from(image.xhot) - ox;
        let top = i32::from(image.y) - i32::from(image.yhot) - oy;
        let (fw, fh) = (frame.width as i32, frame.height as i32);
        for cy in 0..ch {
            let y = top + cy;
            if y < 0 || y >= fh {
                continue;
            }
            for cx in 0..cw {
                let x = left + cx;
                if x < 0 || x >= fw {
                    continue;
                }
                let argb = image.cursor_image[(cy * cw + cx) as usize];
                let alpha = argb >> 24;
                if alpha == 0 {
                    continue;
                }
                let i = ((y * fw + x) * 4) as usize;
                let px = &mut frame.pixels[i..i + 3];
                // BGR from premultiplied ARGB: src + dst * (1 - a).
                for (c, shift) in [(0, 0), (1, 8), (2, 16)] {
                    let src = (argb >> shift) & 0xff;
                    let dst = u32::from(px[c]);
                    px[c] = (src + dst * (255 - alpha) / 255).min(255) as u8;
                }
            }
        }
    }

    fn grab_bare(&mut self) -> Result<Frame, GrabError> {
        let gone = |e: &dyn std::fmt::Display| GrabError::Gone(e.to_string());
        match self.target {
            Target::Area {
                x,
                y,
                width,
                height,
            } => self.image(self.root, x, y, width, height),
            Target::Window(id) => {
                if self.window_hidden(id) {
                    return Err(GrabError::Hidden);
                }
                let geometry = self
                    .conn
                    .get_geometry(id)
                    .map_err(|e| gone(&e))?
                    .reply()
                    .map_err(|e| gone(&e))?;
                let (width, height) = (geometry.width, geometry.height);
                if width == 0 || height == 0 {
                    return Err(GrabError::Hidden);
                }
                if self.composite
                    && let Some(frame) = self.composited(id, width, height)?
                {
                    return Ok(frame);
                }
                self.image(id, 0, 0, width, height)
            }
        }
    }

    fn window_hidden(&self, id: Window) -> bool {
        self.conn
            .get_property(false, id, self.net_state, AtomEnum::ATOM, 0, 64)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|mut v| v.any(|a| a == self.hidden)))
            .unwrap_or(false)
    }

    /// The window from its frame's off-screen picture (the frame is the redirected window; the
    /// client sits in it at an offset). `None` when the frame is not redirected after all.
    fn composited(
        &mut self,
        id: Window,
        width: u16,
        height: u16,
    ) -> Result<Option<Frame>, GrabError> {
        let gone = |e: &dyn std::fmt::Display| GrabError::Gone(e.to_string());
        // The top-level ancestor: the window manager's frame.
        let mut frame = id;
        loop {
            let tree = self
                .conn
                .query_tree(frame)
                .map_err(|e| gone(&e))?
                .reply()
                .map_err(|e| gone(&e))?;
            if tree.parent == self.root || tree.parent == x11rb::NONE {
                break;
            }
            frame = tree.parent;
        }
        let offset = self
            .conn
            .translate_coordinates(id, frame, 0, 0)
            .map_err(|e| gone(&e))?
            .reply()
            .map_err(|e| gone(&e))?;
        let pixmap = self.conn.generate_id().map_err(|e| gone(&e))?;
        let named = self
            .conn
            .composite_name_window_pixmap(frame, pixmap)
            .map_err(|e| gone(&e))?
            .check();
        if named.is_err() {
            return Ok(None);
        }
        let picture = self.image(pixmap, offset.dst_x, offset.dst_y, width, height);
        let _ = self.conn.free_pixmap(pixmap);
        picture.map(Some)
    }

    fn image(
        &mut self,
        drawable: u32,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    ) -> Result<Frame, GrabError> {
        let gone = |e: &dyn std::fmt::Display| GrabError::Gone(e.to_string());
        let size = usize::from(width) * usize::from(height) * 4;
        if self.shm {
            match self.shm_image(drawable, x, y, width, height, size) {
                Ok(frame) => return Ok(frame),
                Err(err) => {
                    tracing::info!("X11 capture: shared memory failed ({err}); using GetImage");
                    self.shm = false;
                    self.segment = None;
                }
            }
        }
        let reply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                drawable,
                x,
                y,
                width,
                height,
                u32::MAX,
            )
            .map_err(|e| gone(&e))?
            .reply()
            .map_err(|e| gone(&e))?;
        bgrx(reply.depth, reply.data, width, height)
    }

    fn shm_image(
        &mut self,
        drawable: u32,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        size: usize,
    ) -> Result<Frame, String> {
        if self.segment.as_ref().is_none_or(|s| s.map.len() < size) {
            if let Some(old) = self.segment.take() {
                let _ = self.conn.shm_detach(old.seg);
            }
            let seg = self.conn.generate_id().map_err(|e| e.to_string())?;
            let reply = self
                .conn
                .shm_create_segment(seg, u32::try_from(size).map_err(|e| e.to_string())?, false)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?;
            let file = File::from(reply.shm_fd);
            // SAFETY: the X server made this segment for us alone and only writes into it
            // while a ShmGetImage it answers is pending; we read it after the reply.
            #[allow(unsafe_code)]
            let map = unsafe { memmap2::MmapOptions::new().len(size).map(&file) }
                .map_err(|e| e.to_string())?;
            self.segment = Some(Segment { seg, map });
        }
        let segment = self.segment.as_ref().expect("segment");
        let reply = self
            .conn
            .shm_get_image(
                drawable,
                x,
                y,
                width,
                height,
                u32::MAX,
                ImageFormat::Z_PIXMAP.into(),
                segment.seg,
                0,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        if usize::try_from(reply.size).unwrap_or(0) < size {
            return Err(format!("{} bytes for {width}x{height}", reply.size));
        }
        bgrx(reply.depth, segment.map[..size].to_vec(), width, height).map_err(|e| format!("{e:?}"))
    }
}

impl Drop for Grabber {
    fn drop(&mut self) {
        if let Some(segment) = self.segment.take() {
            let _ = self.conn.shm_detach(segment.seg);
            let _ = self.conn.flush();
        }
    }
}

/// A Z-pixmap picture of depth 24 or 32 (4 bytes per pixel, BGRX on little-endian servers).
fn bgrx(depth: u8, pixels: Vec<u8>, width: u16, height: u16) -> Result<Frame, GrabError> {
    let size = usize::from(width) * usize::from(height) * 4;
    if !(depth == 24 || depth == 32) || pixels.len() < size {
        return Err(GrabError::Gone(format!(
            "unsupported picture: depth {depth}, {} bytes for {width}x{height}",
            pixels.len()
        )));
    }
    let mut pixels = pixels;
    pixels.truncate(size);
    Ok(Frame {
        width: u32::from(width),
        height: u32::from(height),
        bgra: true,
        pixels,
    })
}
