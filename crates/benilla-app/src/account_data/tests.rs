//! The sections against files on disk: what the server's digests send, what a download stores,
//! and when a write goes up.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use benilla_protocol::messages::{update_account_data, AccountDataDigest, UpdateAccountData};

use super::*;
use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

/// A state folder of its own for one test, with the env lock held.
struct Home {
    dir: PathBuf,
    _guards: (EnvGuard, EnvGuard),
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Home {
    fn new(name: &str) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let dir =
            std::env::temp_dir().join(format!("benilla-acctdata-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let guards = (
            EnvGuard::set("BENILLA_HOME", dir.to_str().unwrap()),
            EnvGuard::unset("WOW_CAPTURE"),
        );
        Self {
            dir,
            _guards: guards,
            _lock: lock,
        }
    }

    fn put(&self, rel: &str, bytes: &[u8]) {
        let path = self.dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn get(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join(rel)).ok()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn me() -> Identity {
    ("Realm".to_string(), "Probesix".to_string())
}

/// A state with the test's defaults, as the binding sections are written against them.
fn state() -> State {
    State {
        defaults: Some(vec![
            ("W".into(), "MOVEFORWARD".into()),
            ("SPACE".into(), "JUMP".into()),
        ]),
        ..State::default()
    }
}

fn md5(bytes: &[u8]) -> AccountDataDigest {
    sections::digest(Some(bytes))
}

/// The commands a step sent, as (opcode name, section, contents).
fn run(step: impl FnOnce(&mut dyn FnMut(ClientCommand))) -> Vec<(&'static str, u32, Vec<u8>)> {
    let mut sent = Vec::new();
    step(&mut |c| {
        sent.push(match c {
            ClientCommand::RequestAccountData { section } => ("request", section, Vec::new()),
            ClientCommand::UpdateAccountData { section, contents } => ("update", section, contents),
            other => panic!("not an account-data send: {other:?}"),
        })
    });
    sent
}

fn cache_file(home: &Home) -> Vec<AccountDataDigest> {
    let bytes = home.get("cache.md5").expect("cache.md5 saved");
    assert_eq!(bytes.len(), 128, "eight digests of 16 bytes");
    bytes.chunks(16).map(|c| c.try_into().unwrap()).collect()
}

/// A login against a server with no copy: every file here goes up as the reference's file, the
/// cache takes each local digest and is saved, and nothing is left pending.
#[test]
fn a_server_with_nothing_is_sent_every_file_and_the_cache_is_saved() {
    let home = Home::new("fresh");
    home.put(
        "macros/account.txt",
        b"MACRO 1 \"Hi\" Ability_Ambush\n/say hi\nEND\n",
    );
    home.put(
        "chat/Realm-Probesix.txt",
        b"# benilla chat cache\n# benilla-writer 2\nVERSION 2\nWINDOW 1\nEND\n",
    );
    home.put(
        "bindings/account.txt",
        b"# benilla key bindings\nbind JUMP F\n",
    );
    let mut s = state();
    let sent = run(|send| s.reconcile(&[[0; 16]; 8], Some(me()), send));
    assert_eq!(
        sent,
        vec![
            (
                "update",
                2,
                b"bind W MOVEFORWARD\r\nbind F JUMP\r\n".to_vec(),
            ),
            (
                "update",
                4,
                b"MACRO 1 \"Hi\" Ability_Ambush\n/say hi\nEND\n".to_vec()
            ),
            ("update", 7, b"VERSION 2\nWINDOW 1\nEND\n".to_vec()),
        ],
        "the bindings as the whole table, the chat file without benilla's notes"
    );
    assert_eq!(s.pending, [false; 8]);
    let cache = cache_file(&home);
    assert_eq!(cache[2], md5(b"bind W MOVEFORWARD\r\nbind F JUMP\r\n"));
    assert_eq!(cache[7], md5(b"VERSION 2\nWINDOW 1\nEND\n"));
    assert_eq!(cache[6], [0; 16], "no file, no digest");
}

/// A file unchanged since the last agreement meets a different server copy: it is asked for and
/// left pending, the cache takes the server's digest; a changed one goes up over the server's.
#[test]
fn an_unchanged_file_asks_for_the_servers_and_a_changed_one_overrides_it() {
    let home = Home::new("ask");
    let layout = b"Frame: ChatFrame1\nFrameLevel: 1\n";
    home.put("layout/Realm-Probesix.txt", layout);
    home.put(
        "macros/Realm-Probesix.txt",
        b"MACRO 16777217 \"Me\" X\nEND\n",
    );
    let mut cached = [[0u8; 16]; 8];
    cached[6] = md5(layout);
    cached[5] = [0x55; 16];
    home.put("cache.md5", cached.as_flattened());
    let mut server = [[0u8; 16]; 8];
    server[6] = [0x66; 16];
    server[5] = [0x77; 16];
    server[1] = [0x11; 16];
    let mut s = state();
    let sent = run(|send| s.reconcile(&server, Some(me()), send));
    assert_eq!(
        sent,
        vec![
            ("request", 1, Vec::new()),
            ("update", 5, b"MACRO 16777217 \"Me\" X\nEND\n".to_vec()),
            ("request", 6, Vec::new()),
        ]
    );
    assert!(
        s.pending[1] && s.pending[6],
        "a request leaves its section pending"
    );
    assert!(!s.pending[5]);
    let cache = cache_file(&home);
    assert_eq!(
        cache[6], [0x66; 16],
        "an unchanged file caches the server's digest"
    );
    assert_eq!(cache[1], [0x11; 16]);
    assert_eq!(cache[5], md5(b"MACRO 16777217 \"Me\" X\nEND\n"));
}

/// The character's bindings were dropped here since they last agreed: their file is empty, as the
/// reference's (`0x4b65c6`), so they go up empty, which the server takes as no copy.
#[test]
fn a_deleted_character_set_goes_up_empty() {
    let home = Home::new("erase");
    home.put("bindings/Realm-Probesix.wtf", b"");
    let mut cached = [[0u8; 16]; 8];
    cached[3] = [0x33; 16];
    home.put("cache.md5", cached.as_flattened());
    let mut server = [[0u8; 16]; 8];
    server[3] = [0x33; 16];
    let mut s = state();
    let sent = run(|send| s.reconcile(&server, Some(me()), send));
    assert_eq!(sent, vec![("update", 3, Vec::new())]);
    assert_eq!(cache_file(&home)[3], md5(b""), "the empty file's digest");
    assert_eq!(
        update_account_data(3, &[]),
        vec![3, 0, 0, 0, 0, 0, 0, 0],
        "a zero size, the server's erase (`MiscHandler.cpp:814`)"
    );
}

/// With no character the per-character sections are left alone on both sides.
#[test]
fn no_character_leaves_the_character_sections_alone() {
    let home = Home::new("nochar");
    home.put("macros/account.txt", b"MACRO 1 \"A\" X\nEND\n");
    let mut server = [[0x44u8; 16]; 8];
    server[4] = [0; 16];
    let mut s = state();
    let sent = run(|send| s.reconcile(&server, None, send));
    assert_eq!(
        sent,
        vec![
            ("request", 0, Vec::new()),
            ("request", 2, Vec::new()),
            ("update", 4, b"MACRO 1 \"A\" X\nEND\n".to_vec()),
        ]
    );
    assert_eq!(cache_file(&home)[5], [0; 16], "untouched");
}

fn download(section: u32, file: &[u8]) -> UpdateAccountData {
    let body = update_account_data(section, file);
    UpdateAccountData {
        section,
        size: file.len() as u32,
        stream: body[8..].to_vec(),
    }
}

/// A download stores the file, clears the pending byte and owes the section's held reader once
/// (`0x5afd01`-`0x5afd19`); the bindings arrive as the reference's table and are kept as
/// benilla's diff.
#[test]
fn a_download_stores_the_file_and_owes_its_reader() {
    let home = Home::new("down");
    let data = AccountData::default();
    data.lock().identity = Some(me());
    assert!(!data.register(6), "pending: the reader is held");
    data.lock()
        .arrive(&download(6, b"Frame: ChatFrame1\r\nX: 10\r\n"));
    assert_eq!(
        home.get("layout/Realm-Probesix.txt").as_deref(),
        Some(&b"Frame: ChatFrame1\r\nX: 10\r\n"[..])
    );
    assert!(!data.lock().pending[6]);
    assert!(data.owed(6));
    assert!(data.take_owed(6));
    assert!(!data.take_owed(6), "the reader is owed once");
    assert!(
        data.register(6),
        "settled: a reader registered now runs at once"
    );

    *data.lock() = State {
        identity: Some(me()),
        ..state()
    };
    let table = b"bind W MOVEFORWARD\r\nbind G JUMP\r\n";
    data.lock().arrive(&download(3, table));
    let diff = String::from_utf8(home.get("bindings/Realm-Probesix.txt").unwrap()).unwrap();
    assert!(diff.contains("bind JUMP G\n"), "{diff}");
    assert!(!diff.contains("MOVEFORWARD"), "a default kept is no line");
    assert_eq!(
        home.get("bindings/Realm-Probesix.wtf").as_deref(),
        Some(&table[..]),
        "the section's own file is the server's bytes"
    );
    assert_eq!(
        sections::read(3, Some(&me()), &[]).as_deref(),
        Some(&table[..]),
        "so its digest is the server's at the next login"
    );

    // A character table binding nothing is the account set in use: the character's diff goes.
    data.lock().arrive(&download(3, b""));
    data.lock().arrive(&download(3, b"\r\n"));
    assert_eq!(home.get("bindings/Realm-Probesix.txt"), None);
    assert_eq!(
        home.get("bindings/Realm-Probesix.wtf").as_deref(),
        Some(&b"\r\n"[..])
    );
}

/// A size of 0 writes nothing (`0x5afc87`) yet ends the wait and runs the held reader
/// (`0x5afd19`); a stream that does not inflate writes nothing either; a section past the table
/// is dropped.
#[test]
fn an_empty_or_broken_download_writes_nothing_and_ends_the_wait() {
    let home = Home::new("empty");
    home.put("chat/Realm-Probesix.txt", b"VERSION 2\n");
    let mut s = state();
    s.identity = Some(me());
    s.arrive(&UpdateAccountData {
        section: 7,
        size: 0,
        stream: Vec::new(),
    });
    assert!(!s.pending[7]);
    assert!(s.owed[7].is_none(), "no reader registered, none owed");
    s.readers[7] = true;
    s.arrive(&UpdateAccountData {
        section: 7,
        size: 0,
        stream: Vec::new(),
    });
    assert!(
        s.owed[7].take().is_some(),
        "the held reader runs on the file it has"
    );
    assert_eq!(
        home.get("chat/Realm-Probesix.txt").as_deref(),
        Some(&b"VERSION 2\n"[..])
    );
    s.arrive(&UpdateAccountData {
        section: 7,
        size: 4,
        stream: vec![1, 2, 3],
    });
    assert!(
        s.owed[7].take().is_some(),
        "and on a stream that did not inflate"
    );
    s.arrive(&download(8, b"x"));
    assert_eq!(s.owed, [None; 8]);
}

fn settled(home: &Path) -> AccountData {
    let _ = home;
    let data = AccountData::default();
    {
        let mut s = data.lock();
        *s = state();
        s.identity = Some(me());
        s.pending = [false; 8];
    }
    data
}

/// Only the interface's shutdown writes the sections through `0x5afed0`, the one caller of the
/// timer's arm (`0x5aff30` → `0x5aff40`): chat, macros, bindings and layout at `0x490c55`,
/// `0x490c5a`, `0x490c64` and `0x490c79`. A save mid-session arms nothing; the shutdown arms the
/// chat and the layout, which it always writes, and what was saved since the last, unless pending.
#[test]
fn only_the_interface_shutdown_arms_an_upload() {
    let home = Home::new("shutdown");
    let data = settled(&home.dir);
    data.saved(&[4, 5]);
    assert!(
        (0..8).all(|s| !data.armed_for_test(s)),
        "a mid-session save arms nothing"
    );
    assert!(data.saved_for_test(4) && data.saved_for_test(5));
    data.lock().pending[5] = true;
    data.shut_down_ui();
    let armed: Vec<usize> = (0..8).filter(|&s| data.armed_for_test(s)).collect();
    assert_eq!(
        armed,
        [4, 6, 7],
        "a pending section's write is skipped (`0x5afee8`)"
    );
    assert!(!data.saved_for_test(4), "the shutdown wrote it");

    run(|send| data.lock().fire(None, send));
    data.shut_down_ui();
    let armed: Vec<usize> = (0..8).filter(|&s| data.armed_for_test(s)).collect();
    assert_eq!(
        armed,
        [6, 7],
        "nothing saved since: the chat and the layout alone"
    );
}

/// The armed timer uploads 30 s after the shutdown when the digest left the cache, then caches and
/// saves it, and a logout's flush runs every armed timer at once.
#[test]
fn a_write_goes_up_thirty_seconds_later_when_it_changed_the_file() {
    let home = Home::new("timer");
    let data = settled(&home.dir);
    data.saved(&[4]);
    data.shut_down_ui();
    assert!(data.lock().timers[4].is_some());

    home.put("macros/account.txt", b"MACRO 2 \"B\" X\nEND\n");
    let soon = Instant::now() + Duration::from_secs(29);
    assert!(run(|send| data.lock().fire(Some(soon), send)).is_empty());
    let later = Instant::now() + Duration::from_secs(31);
    assert_eq!(
        run(|send| data.lock().fire(Some(later), send)),
        vec![("update", 4, b"MACRO 2 \"B\" X\nEND\n".to_vec())]
    );
    assert!(data.lock().timers[4].is_none());
    assert_eq!(cache_file(&home)[4], md5(b"MACRO 2 \"B\" X\nEND\n"));

    // Written again, unchanged: the timer runs and sends nothing.
    data.saved(&[4]);
    data.shut_down_ui();
    assert!(run(|send| data.lock().fire(None, send)).is_empty());

    // Changed and armed: the flush sends it now, not 30 s on.
    home.put("macros/account.txt", b"MACRO 3 \"C\" X\nEND\n");
    data.saved(&[4]);
    data.shut_down_ui();
    let (tx, rx) = crossbeam_channel::unbounded();
    data.flush(&NetCommands(tx));
    assert!(matches!(
        rx.try_recv(),
        Ok(ClientCommand::UpdateAccountData { section: 4, .. })
    ));
}

/// Leaving the world drops the timers unrun and makes every section pending again.
#[test]
fn leaving_the_world_drops_the_timers_and_pends_every_section() {
    let home = Home::new("leave");
    let data = settled(&home.dir);
    data.saved(&[4]);
    data.shut_down_ui();
    data.saved(&[5]);
    data.owe_reader_for_test(6);
    data.lock().reset();
    assert!(!data.saved_for_test(5));
    let s = data.lock();
    assert_eq!(s.pending, [true; 8]);
    assert_eq!(s.timers, [None; 8]);
    assert_eq!(s.owed, [None; 8]);
    assert_eq!(s.readers, [false; 8], "the next load registers its own");
    assert!(s.identity.is_none());
    assert!(s.defaults.is_some(), "the defaults outlive the session");
}

/// `0x5af880`: a missing file leaves the digests as they were; a short one zeroes what it lacks.
#[test]
fn the_cache_file_reads_as_the_reference_reads_it() {
    let home = Home::new("cache");
    let mut cache = [[0x99u8; 16]; 8];
    read_cache(&mut cache);
    assert_eq!(cache, [[0x99; 16]; 8], "no file: unchanged");
    home.put("cache.md5", &[0xab; 20]);
    read_cache(&mut cache);
    assert_eq!(cache[0], [0xab; 16]);
    assert_eq!(&cache[1][..4], &[0xab; 4]);
    assert_eq!(&cache[1][4..], &[0; 12]);
    assert_eq!(cache[7], [0; 16]);
}

/// A chat section from the server is stored as one of benilla's own writer's files, which the
/// one-time repair of the old writer's damage leaves alone, and still reads back as the server's
/// bytes, so its digest is the server's.
#[test]
fn a_chat_section_from_the_server_is_kept_as_an_undamaged_file() {
    let home = Home::new("chat");
    let mut s = state();
    s.identity = Some(me());
    let file = b"VERSION 2\nWINDOW 1\nNAME General\nEND\n";
    s.arrive(&download(7, file));
    let kept = home.get("chat/Realm-Probesix.txt").unwrap();
    assert!(
        String::from_utf8_lossy(&kept).contains(crate::ui_chat::settings::WRITER_GENERATION),
        "marked: {}",
        String::from_utf8_lossy(&kept)
    );
    assert_eq!(
        sections::read(7, Some(&me()), &[]).as_deref(),
        Some(&file[..]),
        "the section is the server's bytes"
    );
}

/// The check settles every section it does not ask for (`0x5afa42`) and runs each held reader
/// there (`0x5afab1`-`0x5afaca`); a section it asks for stays pending, its reader held for the
/// download.
#[test]
fn the_check_runs_the_held_readers_of_the_sections_it_settles() {
    let home = Home::new("held");
    let layout = b"Frame: ChatFrame1\nFrameLevel: 1\n";
    home.put("layout/Realm-Probesix.txt", layout);
    let mut cached = [[0u8; 16]; 8];
    cached[6] = md5(layout);
    home.put("cache.md5", cached.as_flattened());
    let mut server = [[0u8; 16]; 8];
    server[6] = [0x66; 16];
    let mut s = state();
    s.readers = [true; 8];
    run(|send| s.reconcile(&server, Some(me()), send));
    assert!(s.pending[6] && s.owed[6].is_none(), "asked for: held");
    for section in [0, 1, 2, 3, 4, 5, 7] {
        assert!(!s.pending[section]);
        assert!(s.owed[section].is_some(), "section {section}'s reader runs");
    }
    // With no state folder nothing is checked, so nothing waits.
    drop(home);
    let _lock = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let _capture = EnvGuard::set("WOW_CAPTURE", "1");
    assert_eq!(State::default().pending, [false; 8]);
}

/// An account table from another client, in its own order: kept as it came, so the next login's
/// check finds the digests equal and sends nothing.
#[test]
fn a_downloaded_section_is_kept_as_the_server_sent_it() {
    let home = Home::new("roundtrip");
    let table = b"bind SPACE JUMP\r\nbind W MOVEFORWARD\r\nbind F JUMP\r\n";
    let mut s = state();
    s.identity = Some(me());
    s.arrive(&download(2, table));
    assert_eq!(
        home.get("bindings/account.wtf").as_deref(),
        Some(&table[..])
    );
    let mut server = [[0u8; 16]; 8];
    server[2] = md5(table);
    let mut cached = [[0u8; 16]; 8];
    cached[2] = md5(table);
    home.put("cache.md5", cached.as_flattened());
    let mut next = state();
    assert!(
        run(|send| next.reconcile(&server, Some(me()), send)).is_empty(),
        "nothing changed on either side"
    );
}
