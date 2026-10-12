use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

/// Short enough that the timeout test is quick.
const FAST: Timeouts = Timeouts {
    connect: Duration::from_millis(500),
    read: Duration::from_millis(300),
};

// ── The buffer, chunk by chunk ──

#[test]
fn a_chunk_loses_its_last_byte_and_stops_at_a_nul() {
    let mut b = AlertBuffer::default();
    b.push(b"SERVERALERT:abX");
    assert_eq!(b.buf, b"SERVERALERT:ab");
    // One byte leaves nothing (`SStrCopy` of size 1), and an empty read adds nothing.
    let mut b = AlertBuffer::default();
    b.push(b"X");
    b.push(b"");
    assert!(b.buf.is_empty());
    let mut b = AlertBuffer::default();
    b.push(b"ab\0cdX");
    assert_eq!(b.buf, b"ab");
}

/// The callback appends (`SStrPack`), so a body read in pieces is the pieces joined, each short
/// of its last byte.
#[test]
fn chunks_add_up_to_the_buffer_and_stop_at_its_cap() {
    let mut b = AlertBuffer::default();
    b.push(b"SERVERALERT:abX");
    b.push(b"cdY");
    assert_eq!(b.finish().as_deref(), Some(&b"abcd"[..]));

    let mut b = AlertBuffer::default();
    b.push(b"SERVERALERT:");
    b.push(&vec![b'a'; 0x4000]);
    assert_eq!(b.buf.len(), 0x7ff, "0x800 with the terminator");
    assert!(b.is_full());
    b.push(b"more");
    assert_eq!(b.buf.len(), 0x7ff);
    // A chunk of exactly 0x7ff or 0x800 bytes fills it the same way.
    for len in [0x7ff, 0x800, 0x801] {
        let mut b = AlertBuffer::default();
        b.push(&vec![b'a'; len]);
        assert_eq!(b.buf.len(), (len.min(0x800) - 1), "{len}");
    }
}

#[test]
fn the_prefix_is_case_folded_and_must_be_all_there() {
    let alert = |body: &[u8]| {
        let mut b = AlertBuffer::default();
        b.push(body);
        b.finish()
    };
    assert_eq!(alert(b"serverAlert:hi\n"), Some(b"hi".to_vec()));
    assert_eq!(alert(b"SERVERALERT:\n"), Some(Vec::new()));
    assert_eq!(alert(b"SERVERALERT\n"), None, "no colon");
    assert_eq!(alert(b"SERVERALERT"), None, "a prefix cut short");
    assert_eq!(alert(b" SERVERALERT:hi\n"), None, "not at the start");
    assert_eq!(alert(b"<html>SERVERALERT:</html>"), None);
    assert_eq!(alert(b""), None);
}

// ── The URL ──

#[test]
fn urls_split_into_host_port_and_target() {
    let url = |s| parse_url(s).unwrap();
    assert_eq!(
        url("http://launcher.worldofwarcraft.com/alert"),
        Url {
            host: "launcher.worldofwarcraft.com".into(),
            port: 80,
            target: "/alert".into()
        }
    );
    assert_eq!(url("HTTP://h:8080").target, "/");
    assert_eq!(url("http://h:8080?x=1#frag").target, "/?x=1");
    assert_eq!(url("http://h:8080?x=1").port, 8080);
    assert_eq!(url("http://u:p@h/a/b?c#d").host, "h");
    let v6 = url("http://[::1]:81/p");
    assert_eq!((v6.host.as_str(), v6.port), ("::1", 81));
    assert!(matches!(parse_url("https://h/"), Err(FetchError::Https)));
    assert!(matches!(parse_url("HTTPS://h/"), Err(FetchError::Https)));
    for bad in [
        "",
        "ftp://h/",
        "h/alert",
        "http://",
        "http://:80/",
        "http://h:x/",
    ] {
        assert!(matches!(parse_url(bad), Err(FetchError::BadUrl)), "{bad}");
    }
}

#[test]
fn a_location_resolves_against_the_request() {
    let base = parse_url("http://h:81/a/b?q").unwrap();
    let go = |l| resolve(&base, l).unwrap();
    assert_eq!(go("/c").target, "/c");
    assert_eq!(go("c").target, "/a/c");
    assert_eq!(go("http://other/x").host, "other");
    assert_eq!(go("//other:9/x").port, 9);
    assert_eq!((go("/c").host.as_str(), go("/c").port), ("h", 81));
    assert!(matches!(
        resolve(&base, "https://h/"),
        Err(FetchError::Https)
    ));
}

// ── The fetch, against a fake server ──

/// What a fake server does with a connection.
enum Reply {
    /// Write these bytes and close.
    Raw(Vec<u8>),
    /// Accept and say nothing.
    Silent,
    /// Write these bytes and then say nothing, the connection open.
    Stall(Vec<u8>),
}

/// A server on a loopback port: each connection's request head goes to `handler` with the
/// connection's number, and the heads are kept.
struct Fake {
    port: u16,
    heads: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn start(handler: impl Fn(&str, usize, u16) -> Reply + Send + 'static) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let seen = heads.clone();
        std::thread::spawn(move || {
            for (n, conn) in listener.incoming().enumerate() {
                let Ok(mut conn) = conn else { return };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if conn.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let reply = handler(&head, n, port);
                seen.lock().unwrap().push(head);
                match reply {
                    Reply::Raw(bytes) => {
                        let _ = conn.write_all(&bytes);
                    }
                    Reply::Silent => std::thread::sleep(Duration::from_secs(2)),
                    Reply::Stall(bytes) => {
                        let _ = conn.write_all(&bytes);
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        });
        Fake { port, heads }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn requests(&self) -> usize {
        self.heads.lock().unwrap().len()
    }
}

fn ok(body: &[u8]) -> Reply {
    let mut raw = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    raw.extend_from_slice(body);
    Reply::Raw(raw)
}

fn alert(url: &str) -> Option<String> {
    fetch_alert(url, FAST).map(|t| String::from_utf8_lossy(&t).into_owned())
}

#[test]
fn a_200_with_the_prefix_is_an_alert_and_the_request_is_the_references() {
    let fake = Fake::start(|_, _, _| ok(b"SERVERALERT:\nRealms are down.\n"));
    assert_eq!(
        alert(&fake.url("/alert?x=1")).as_deref(),
        // The last byte, the final newline, is the callback's to lose.
        Some("\nRealms are down.")
    );
    let head = fake.heads.lock().unwrap()[0].clone();
    assert!(head.starts_with("GET /alert?x=1 HTTP/1.1\r\n"), "{head}");
    assert!(head.contains(&format!("Host: 127.0.0.1:{}\r\n", fake.port)));
    assert!(head.contains("User-Agent: Blizzard Web Client\r\n"));
}

#[test]
fn a_200_without_the_prefix_is_no_alert() {
    let fake = Fake::start(|_, _, _| ok(b"<html>Not found, really.</html>\n"));
    assert_eq!(alert(&fake.url("/")), None);
}

#[test]
fn a_404_is_no_alert_even_with_the_prefix_in_its_body() {
    let body = b"SERVERALERT:\nnot for you\n";
    let fake = Fake::start(move |_, _, _| {
        let mut raw = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        raw.extend_from_slice(body);
        Reply::Raw(raw)
    });
    assert_eq!(alert(&fake.url("/")), None);
}

#[test]
fn redirects_are_followed_up_to_the_limit() {
    let fake = Fake::start(|head, _, port| {
        let to = |loc: String| {
            Reply::Raw(
                format!("HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\n\r\n")
                    .into_bytes(),
            )
        };
        if head.starts_with("GET /a ") {
            to("/b".into())
        } else if head.starts_with("GET /b ") {
            to(format!("http://127.0.0.1:{port}/c"))
        } else if head.starts_with("GET /loop ") {
            to("/loop".into())
        } else {
            ok(b"SERVERALERT:moved\n")
        }
    });
    assert_eq!(alert(&fake.url("/a")).as_deref(), Some("moved"));
    assert_eq!(fake.requests(), 3);
    let before = fake.requests();
    assert_eq!(alert(&fake.url("/loop")), None);
    assert_eq!(fake.requests() - before, MAX_REDIRECTS + 1);
}

#[test]
fn a_redirect_to_https_is_not_followed() {
    let fake = Fake::start(|_, _, _| {
        Reply::Raw(
            b"HTTP/1.1 301 Moved\r\nLocation: https://127.0.0.1:1/alert\r\nContent-Length: 0\r\n\r\n"
                .to_vec(),
        )
    });
    assert!(matches!(
        download(&fake.url("/"), FAST, &mut |_| true),
        Err(FetchError::Https)
    ));
    assert_eq!(alert(&fake.url("/")), None);
}

#[test]
fn a_chunked_body_is_decoded() {
    let fake = Fake::start(|_, _, _| {
        Reply::Raw(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
              7\r\nSERVERA\r\n8;ext=1\r\nLERT:hi!\r\n3\r\n\nyo\r\n0\r\nTrailer: x\r\n\r\n"
                .to_vec(),
        )
    });
    // One read of the whole body: the decoded bytes, short of the last.
    assert_eq!(alert(&fake.url("/")).as_deref(), Some("hi!\ny"));
}

#[test]
fn a_body_with_no_length_is_read_to_the_close() {
    let fake = Fake::start(|_, _, _| {
        Reply::Raw(b"HTTP/1.0 200 OK\r\nServer: x\r\n\r\nSERVERALERT:to the end\n".to_vec())
    });
    assert_eq!(alert(&fake.url("/")).as_deref(), Some("to the end"));
}

#[test]
fn a_server_that_says_nothing_times_out() {
    let fake = Fake::start(|_, _, _| Reply::Silent);
    let start = Instant::now();
    assert_eq!(alert(&fake.url("/")), None);
    assert!(start.elapsed() < Duration::from_millis(1500));
}

/// The status and the first bytes, then silence past the read timeout: the done call
/// (`0x7a6c4b`) runs whatever ended the reads, and finds the prefix.
#[test]
fn a_body_that_stalls_keeps_what_came_before_it() {
    let fake = Fake::start(|_, _, _| {
        Reply::Stall(
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nSERVERALERT:partial\nmore".to_vec(),
        )
    });
    let start = Instant::now();
    assert_eq!(alert(&fake.url("/")).as_deref(), Some("partial\nmor"));
    assert!(start.elapsed() < Duration::from_millis(1500));
}

#[test]
fn a_3_kb_body_fills_the_buffer_and_no_more() {
    let mut body = b"SERVERALERT:".to_vec();
    body.extend((0..3000).map(|i| b'a' + (i % 26) as u8));
    let fake = Fake::start(move |_, _, _| ok(&body.clone()));
    let text = fetch_alert(&fake.url("/"), FAST).unwrap();
    assert_eq!(text.len(), 0x7ff - PREFIX.len());
    assert!(text.starts_with(b"abcdefghij"));
}

#[test]
fn an_https_url_is_never_dialled() {
    let fake = Fake::start(|_, _, _| ok(b"SERVERALERT:x"));
    let https = fake.url("/").replacen("http://", "https://", 1);
    assert!(matches!(
        download(&https, FAST, &mut |_| true),
        Err(FetchError::Https)
    ));
    assert_eq!(alert(&https), None);
    assert_eq!(fake.requests(), 0);
}

#[test]
fn a_refused_connection_is_silent() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    assert_eq!(alert(&format!("http://127.0.0.1:{port}/")), None);
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

/// The fetch runs on a thread of its own, so the caller never waits on it.
#[test]
fn the_background_fetch_hands_over_its_alert_through_a_channel() {
    let fake = Fake::start(|_, _, _| ok(b"SERVERALERT:\nhello\n"));
    let rx = spawn_fetch(fake.url("/"));
    let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(got.as_deref(), Some(&b"\nhello"[..]));
}

/// A test run starts no fetch and opens no browser, whatever the URL.
#[test]
fn a_test_run_neither_fetches_nor_opens_a_browser() {
    assert!(!allowed());
    assert!(begin_once("http://127.0.0.1:1/").is_none());
}

/// A writer that records each call, to see how the request leaves.
#[derive(Default)]
struct Calls(Vec<Vec<u8>>);

impl Write for Calls {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.push(buf.to_vec());
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_request_head_leaves_in_one_write() {
    let mut calls = Calls::default();
    send_request(&mut calls, &parse_url("http://h:81/a?b").unwrap()).unwrap();
    assert_eq!(calls.0.len(), 1);
    let head = String::from_utf8(calls.0.remove(0)).unwrap();
    assert!(
        head.starts_with("GET /a?b HTTP/1.1\r\nHost: h:81\r\n"),
        "{head}"
    );
    assert!(head.ends_with("\r\n\r\n"));
}

#[test]
fn an_ipv6_host_is_bracketed_in_the_host_header() {
    let mut calls = Calls::default();
    send_request(&mut calls, &parse_url("http://[::1]/").unwrap()).unwrap();
    let head = String::from_utf8(calls.0.concat()).unwrap();
    assert!(head.contains("\r\nHost: [::1]\r\n"), "{head}");
    calls.0.clear();
    send_request(&mut calls, &parse_url("http://[::1]:8080/").unwrap()).unwrap();
    assert!(String::from_utf8(calls.0.concat())
        .unwrap()
        .contains("\r\nHost: [::1]:8080\r\n"));
}

/// A chunked body whose trailers never end is refused, not read forever.
#[test]
fn endless_chunked_trailers_are_refused() {
    let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n".to_vec();
    for _ in 0..MAX_HEADERS + 5 {
        raw.extend_from_slice(b"X-Trailer: y\r\n");
    }
    raw.extend_from_slice(b"\r\n");
    let fake = Fake::start(move |_, _, _| Reply::Raw(raw.clone()));
    assert!(matches!(
        download(&fake.url("/"), FAST, &mut |_| true),
        Err(FetchError::Io(_))
    ));
    // The ordinary trailer still ends it.
    let fake = Fake::start(|_, _, _| {
        Reply::Raw(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX: y\r\n\r\n".to_vec(),
        )
    });
    assert!(download(&fake.url("/"), FAST, &mut |_| true).is_ok());
}
