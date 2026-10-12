//! The server alert (`SHOW_SERVER_ALERT`): the one background fetch of the text the login screen
//! shows over its boxes, and the verb that opens a link in it.
//!
//! The glue manager's init (`0x46a400`) reads the Lua global `SERVER_ALERT_URL` by name (`0x46a541`
//! → `0x703bf0`, after the GlueXML load at `0x46a505`, so `GlueLocalization` overrides the stock
//! `GlueStrings` value), sets `alertPending` and clears `alertReady` and the buffer
//! (`0xb41e28`, `0xb41e2c`, `0xb414f8`), and starts the download once (`0x46a564` → `0x7a6ee0`):
//! `GET`, agent `Blizzard Web Client` (`0x87d3b0`), a 5000 ms connect and receive timeout
//! (`0x7a6ee6`), redirects followed. Only a 2xx status reads the body (`0x7a6a0e`-`0x7a6a21`); each
//! read of up to 0x4000 bytes (`0x7a6a56`) reaches the callback (`0x46a580`) with its size, and the
//! callback appends it to the 0x800-byte buffer ([`AlertBuffer`]). When the connection handle
//! closes (`[esi+4]` at `0x7a6be3`, the `InternetConnect` handle of `0x7a6ddf`), the callback runs
//! once more with the done flag (`0x7a6c4b`), and a buffer that begins `SERVERALERT:` case-folded
//! (`0x836ed8`) sets `alertReady` (`0x46a5f1`). Every failure is silent, and nothing refetches.
//!
//! An `https://` URL shows nothing in the reference either: `InternetConnect` asks for the HTTP
//! service (`0x7a6dcf`) and `HttpOpenRequest` passes `0x84000200` (`0x7a6e34`), without
//! `INTERNET_FLAG_SECURE`, so it speaks plain HTTP to the URL's port (443 for `https://`) and gets
//! no 2xx. benilla does not dial an `https://` URL, with the same result, and logs one line. The
//! client sets neither redirect flag (`0x4000`, `0x8000`); what WinINet does with an
//! http-to-https redirect is the OS's, not in the client's bytes, and benilla does not follow one.
//!
//! Deviation: the body is read with 0x4000-byte reads filled to the end of the body, so what a
//! read returns does not depend on how the network cut the packets; and the reading stops once
//! the buffer is full, since no later byte can change it.
//! Deviation: the connection is direct. The reference opens its session with access type 0
//! (`InternetOpenA`, `0x7a6d97`), the system's proxy settings, which benilla does not read.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use bevy::prelude::*;

/// What `0x46b957` skips to find the text: the prefix `0x836ed8` names (`0x836b3c` points at it).
const PREFIX: &[u8] = b"SERVERALERT:";
/// The buffer `0xb414f8` and the stack copy in the callback, both 0x800 bytes with their NUL.
const BUFFER: usize = 0x800;
/// One read (`0x7a6a56`).
const CHUNK: usize = 0x4000;
/// The user agent (`0x87d3b0`).
const AGENT: &str = "Blizzard Web Client";
/// Redirects followed before the fetch gives up.
const MAX_REDIRECTS: usize = 5;
/// A status line or header line past this is not an HTTP response.
const MAX_LINE: usize = 8192;
const MAX_HEADERS: usize = 100;

/// The connect and receive timeouts, `0x1388` ms each (`0x7a6ee6`).
#[derive(Clone, Copy, Debug)]
pub(super) struct Timeouts {
    pub connect: Duration,
    pub read: Duration,
}

impl Timeouts {
    pub(super) const REFERENCE: Timeouts = Timeouts {
        connect: Duration::from_millis(5000),
        read: Duration::from_millis(5000),
    };
}

/// The callback's buffer (`0xb414f8`): what the chunks of a response add up to.
#[derive(Debug, Default)]
pub(super) struct AlertBuffer {
    buf: Vec<u8>,
}

impl AlertBuffer {
    /// One read's bytes (`0x46a580`). An empty read adds nothing. Otherwise the callback copies the
    /// chunk into a 0x800-byte stack string with `SStrCopy` (`0x64a5a0`, size `min(len, 0x800)`),
    /// which stops at a NUL and leaves room for its own, so the chunk's last byte is lost; then it
    /// appends that to the buffer with `SStrPack` (`0x64a760`, size 0x800), so chunks add up to
    /// 0x7ff bytes and no more.
    pub(super) fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let piece = &chunk[..chunk.len().min(BUFFER) - 1];
        let piece = &piece[..piece.iter().position(|&b| b == 0).unwrap_or(piece.len())];
        let room = (BUFFER - 1).saturating_sub(self.buf.len());
        self.buf.extend_from_slice(&piece[..piece.len().min(room)]);
    }

    /// Nothing a later chunk adds can change the buffer.
    pub(super) fn is_full(&self) -> bool {
        self.buf.len() >= BUFFER - 1
    }

    /// The done call (`0x46a5c0`): the text after the prefix when the buffer begins with it, any
    /// case (`SStrCmpI` over the prefix's length, `0x46a5e8`); a shorter or different buffer is no
    /// alert.
    pub(super) fn finish(self) -> Option<Vec<u8>> {
        let head = self.buf.get(..PREFIX.len())?;
        head.eq_ignore_ascii_case(PREFIX)
            .then(|| self.buf[PREFIX.len()..].to_vec())
    }
}

/// Why a download ended without its body.
#[derive(Debug)]
pub(super) enum FetchError {
    /// Not an `http://` or `https://` URL, or one with no host.
    BadUrl,
    /// An `https://` URL, the deviation above.
    Https,
    /// A status that is neither 2xx nor a redirect with a `Location`.
    Status(u16),
    TooManyRedirects,
    /// Not an HTTP response.
    Protocol,
    Io(io::Error),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::BadUrl => f.write_str("not an http URL"),
            FetchError::Https => f.write_str("https"),
            FetchError::Status(status) => write!(f, "status {status}"),
            FetchError::TooManyRedirects => f.write_str("too many redirects"),
            FetchError::Protocol => f.write_str("not an HTTP response"),
            FetchError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for FetchError {
    fn from(e: io::Error) -> Self {
        FetchError::Io(e)
    }
}

/// A parsed `http://` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Url {
    host: String,
    port: u16,
    /// The request target: the path and query, `/` at least.
    target: String,
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &s[prefix.len()..])
}

fn parse_url(url: &str) -> Result<Url, FetchError> {
    let rest = match strip_prefix_ci(url, "http://") {
        Some(rest) => rest,
        None if strip_prefix_ci(url, "https://").is_some() => return Err(FetchError::Https),
        None => return Err(FetchError::BadUrl),
    };
    let rest = rest.split('#').next().unwrap_or_default();
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    // User information is not sent: the request names the host alone.
    let authority = authority.rsplit('@').next().unwrap_or_default();
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']').ok_or(FetchError::BadUrl)?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let port = match port {
        None | Some("") => 80,
        Some(p) => p.parse().map_err(|_| FetchError::BadUrl)?,
    };
    if host.is_empty() {
        return Err(FetchError::BadUrl);
    }
    let target = if tail.is_empty() || tail.starts_with('?') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    Ok(Url {
        host: host.to_string(),
        port,
        target,
    })
}

/// Where a `Location` points from `base`: a full URL, a scheme-less `//host/..`, an absolute path
/// or a path relative to the request's directory.
fn resolve(base: &Url, location: &str) -> Result<Url, FetchError> {
    let location = location.trim();
    if strip_prefix_ci(location, "http://").is_some()
        || strip_prefix_ci(location, "https://").is_some()
    {
        return parse_url(location);
    }
    if let Some(rest) = location.strip_prefix("//") {
        return parse_url(&format!("http://{rest}"));
    }
    let target = if location.starts_with('/') {
        location.to_string()
    } else {
        let path = base.target.split('?').next().unwrap_or("/");
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        format!("{dir}{location}")
    };
    Ok(Url {
        host: base.host.clone(),
        port: base.port,
        target,
    })
}

fn connect(url: &Url, timeouts: Timeouts) -> Result<TcpStream, FetchError> {
    let mut last = None;
    for addr in (url.host.as_str(), url.port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeouts.connect) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeouts.read))?;
                stream.set_write_timeout(Some(timeouts.read))?;
                return Ok(stream);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))
        .into())
}

/// One line without its terminator, `None` at the end of the stream.
fn read_line(r: &mut impl BufRead) -> Result<Option<String>, FetchError> {
    let mut line = Vec::new();
    r.by_ref()
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut line)?;
    if line.is_empty() {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        return Err(FetchError::Protocol);
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Ok(Some(String::from_utf8_lossy(&line).into_owned()))
}

/// A response head: the status and the headers the body needs.
#[derive(Debug, Default)]
struct Head {
    status: u16,
    location: Option<String>,
    content_length: Option<u64>,
    chunked: bool,
}

fn read_head(r: &mut impl BufRead) -> Result<Head, FetchError> {
    let status = read_line(r)?.ok_or(FetchError::Protocol)?;
    let mut parts = status.split_whitespace();
    if !parts.next().is_some_and(|v| v.starts_with("HTTP/")) {
        return Err(FetchError::Protocol);
    }
    let mut head = Head {
        status: parts
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or(FetchError::Protocol)?,
        ..Head::default()
    };
    for _ in 0..MAX_HEADERS {
        let line = read_line(r)?.ok_or(FetchError::Protocol)?;
        if line.is_empty() {
            return Ok(head);
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "location" => head.location = Some(value.to_string()),
            "content-length" => head.content_length = value.parse().ok(),
            "transfer-encoding" => {
                head.chunked = value
                    .split(',')
                    .any(|t| t.trim().eq_ignore_ascii_case("chunked"));
            }
            _ => {}
        }
    }
    Err(FetchError::Protocol)
}

/// `Transfer-Encoding: chunked`, decoded.
struct ChunkedReader<R> {
    inner: R,
    /// Bytes left in the current chunk.
    left: u64,
    /// The previous chunk's trailing CRLF is still to be read.
    crlf: bool,
    done: bool,
}

impl<R: BufRead> ChunkedReader<R> {
    fn new(inner: R) -> Self {
        ChunkedReader {
            inner,
            left: 0,
            crlf: false,
            done: false,
        }
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let bad = |what| io::Error::new(io::ErrorKind::InvalidData, what);
        if self.crlf {
            read_line(&mut self.inner).map_err(|_| bad("chunk end"))?;
            self.crlf = false;
        }
        let line = read_line(&mut self.inner)
            .map_err(|_| bad("chunk size"))?
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let size = line.split(';').next().unwrap_or_default().trim();
        self.left = u64::from_str_radix(size, 16).map_err(|_| bad("chunk size"))?;
        if self.left == 0 {
            // The trailers, up to the blank line.
            let mut ended = false;
            for _ in 0..MAX_HEADERS {
                if read_line(&mut self.inner)
                    .map_err(|_| bad("trailer"))?
                    .is_none_or(|l| l.is_empty())
                {
                    ended = true;
                    break;
                }
            }
            if !ended {
                return Err(bad("trailer"));
            }
            self.done = true;
        } else {
            self.crlf = true;
        }
        Ok(())
    }
}

impl<R: BufRead> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.left == 0 {
            if self.done {
                return Ok(0);
            }
            self.next_chunk()?;
        }
        let want = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// Read until `buf` is full or the body ends. A read error after some bytes returns them with the
/// error.
fn fill(body: &mut impl Read, buf: &mut [u8]) -> (usize, Option<io::Error>) {
    let mut n = 0;
    while n < buf.len() {
        match body.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return (n, Some(e)),
        }
    }
    (n, None)
}

/// The body in reads of up to [`CHUNK`] bytes, each handed to `on_chunk`, which answers whether to
/// read on.
fn stream_body(
    body: &mut impl Read,
    on_chunk: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), FetchError> {
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let (n, error) = fill(body, &mut chunk);
        if n > 0 && !on_chunk(&chunk[..n]) {
            return Ok(());
        }
        if let Some(e) = error {
            return Err(e.into());
        }
        if n < CHUNK {
            return Ok(());
        }
    }
}

/// The request, in one write so the head leaves in one segment. An IPv6 literal is bracketed in
/// the Host header.
fn send_request(w: &mut impl Write, url: &Url) -> io::Result<()> {
    let name = if url.host.contains(':') {
        format!("[{}]", url.host)
    } else {
        url.host.clone()
    };
    let host = if url.port == 80 {
        name
    } else {
        format!("{name}:{}", url.port)
    };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {AGENT}\r\nAccept: */*\r\n\
         Cache-Control: no-cache\r\nPragma: no-cache\r\nConnection: close\r\n\r\n",
        url.target
    );
    w.write_all(request.as_bytes())
}

/// `GET url`, following up to [`MAX_REDIRECTS`] redirects, and the 2xx body to `on_chunk` in reads
/// of up to 0x4000 bytes. A body that ends in an error has handed over what came before it.
pub(super) fn download(
    url: &str,
    timeouts: Timeouts,
    on_chunk: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), FetchError> {
    let mut url = parse_url(url)?;
    for _ in 0..=MAX_REDIRECTS {
        let mut stream = connect(&url, timeouts)?;
        send_request(&mut stream, &url)?;
        let mut reader = BufReader::with_capacity(CHUNK, stream);
        let head = read_head(&mut reader)?;
        match head.status {
            // `0x7a6a11`-`0x7a6a21`: 200 up to 299 reads the body.
            200..=299 => {
                if head.status == 204 {
                    return Ok(());
                }
                return if head.chunked {
                    stream_body(&mut ChunkedReader::new(reader), on_chunk)
                } else if let Some(length) = head.content_length {
                    stream_body(&mut reader.take(length), on_chunk)
                } else {
                    stream_body(&mut reader, on_chunk)
                };
            }
            301 | 302 | 303 | 307 | 308 => {
                let location = head.location.ok_or(FetchError::Status(head.status))?;
                url = resolve(&url, &location)?;
            }
            status => return Err(FetchError::Status(status)),
        }
    }
    Err(FetchError::TooManyRedirects)
}

/// The alert `url` serves: the text after `SERVERALERT:`, or `None` for every other outcome.
pub(super) fn fetch_alert(url: &str, timeouts: Timeouts) -> Option<Vec<u8>> {
    let mut buffer = AlertBuffer::default();
    let result = download(url, timeouts, &mut |chunk| {
        buffer.push(chunk);
        !buffer.is_full()
    });
    match result {
        Err(FetchError::Https) => {
            info!("server alert: {url} is https; the reference shows nothing for it, nor does benilla");
        }
        Err(e) => debug!("server alert: {url}: {e}"),
        Ok(()) => {}
    }
    buffer.finish()
}

/// The fetch on a thread of its own: the channel holds the alert once, or closes without one.
pub(super) fn spawn_fetch(url: String) -> crossbeam_channel::Receiver<Option<Vec<u8>>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let started = std::thread::Builder::new()
        .name("server-alert".into())
        .spawn(move || {
            let _ = tx.send(fetch_alert(&url, Timeouts::REFERENCE));
        });
    if let Err(e) = started {
        debug!("server alert: no thread: {e}");
    }
    rx
}

/// Whether this run may touch the network or the desktop for the alert: not a test, and not a
/// capture or any other unattended run.
pub(super) fn allowed() -> bool {
    !cfg!(test) && !crate::run_mode::unattended()
}

/// Whether the first glue VM of the process has been seen: the reference initialises its glue
/// manager once (`0x46a400`), so the fetch happens once however many glue VMs follow.
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The fetch for the glue manager's init (`0x46a564`): started once per process, and only for a
/// URL that is not empty, when the run [`allowed`] it.
pub(super) fn begin_once(url: &str) -> Option<crossbeam_channel::Receiver<Option<Vec<u8>>>> {
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) || !allowed() || url.is_empty() {
        return None;
    }
    Some(spawn_fetch(url.to_string()))
}

/// `LaunchURL` reaching the host (`0x5abd00`): a URL that passed the verb's whitelist.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchUrl(pub(crate) String);

/// `LaunchAddOnURL` reaching the host (`0x429ff0` direct): a decoded web address
/// ([`benilla_ui::script::launch_addon_url`]).
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchAddonUrl(pub(crate) String);

/// [`open_launched`] for an add-on's URL, which the reference launches with no whitelist
/// (`0x46d5e0`); the one check here is [`benilla_ui::script::launch_addon_url_allowed`].
pub(crate) fn open_addon_launched(mut urls: MessageReader<LaunchAddonUrl>) {
    for LaunchAddonUrl(url) in urls.read() {
        if !benilla_ui::script::launch_addon_url_allowed(url) {
            warn!("LaunchAddOnURL: {url:?} refused");
            continue;
        }
        if !allowed() {
            debug!("LaunchAddOnURL: {url} not opened in this run");
            continue;
        }
        open_in_browser(url);
    }
}

/// Open each launched URL in the player's browser (`0x429ff0`'s `ShellExecute "open"`), never in
/// a test or an unattended run.
pub(crate) fn open_launched(mut urls: MessageReader<LaunchUrl>) {
    for LaunchUrl(url) in urls.read() {
        // The verb gated it, and the opener does not take the message's word for it.
        if !benilla_ui::script::launch_url_allowed(url) {
            warn!("LaunchURL: {url:?} refused");
            continue;
        }
        if !allowed() {
            debug!("LaunchURL: {url} not opened in this run");
            continue;
        }
        open_in_browser(url);
    }
}

/// The platform's opener, detached. The URL is `http://` and letters, digits, `.`, `-` and `/`
/// ([`benilla_ui::script::GlueRequest::LaunchUrl`]), or a web address of URL characters and
/// `%XY` escapes ([`benilla_ui::script::launch_addon_url_allowed`]), and goes as one argument to
/// an opener that is no shell.
fn open_in_browser(url: &str) {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("/usr/bin/open")
    } else if cfg!(target_os = "windows") {
        // By its full path, never a search of the working directory or the PATH.
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let mut c = std::process::Command::new(
            std::path::Path::new(&root)
                .join("System32")
                .join("rundll32.exe"),
        );
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match command.spawn() {
        Ok(mut child) => {
            // Reaped off the main thread, so the opener never lingers as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => warn!("LaunchURL: no browser opener: {e}"),
    }
}

#[cfg(test)]
#[path = "server_alert_tests.rs"]
mod tests;
