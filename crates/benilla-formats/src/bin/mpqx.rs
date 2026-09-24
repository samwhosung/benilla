//! One-off extraction helper (session tool, not shipped): `mpqx <data-dir> <virtual-path> <out>`.
//! Reads through the SAME patch chain as the runtime ([`benilla_formats::Chain`], the full vanilla
//! mount law) — it used to hand-roll a partial archive list in `benilla-mpq` and silently
//! miss whole archives (`wmo.MPQ`: "not found" for every building/ship). A path the top archive
//! **delete-marks** is reported as DELETED (exit 2) — the client doesn't load it; look for a
//! `Blizzard_*` addon replacement.

use std::path::Path;

use benilla_formats::Chain;

fn main() {
    let mut args = std::env::args().skip(1);
    let (data, vpath, out) = (
        args.next().expect("data dir"),
        args.next().expect("virtual path"),
        args.next().expect("out path"),
    );
    let chain = Chain::open(Path::new(&data)).expect("open patch chain");
    match chain.read(&vpath) {
        Ok(bytes) => {
            std::fs::write(&out, &bytes).expect("write");
            let from = chain
                .find_file_archive(&vpath)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            println!(
                "{} -> {} ({} bytes, from {})",
                vpath,
                out,
                bytes.len(),
                from
            );
        }
        // Chain::read distinguishes the tombstone in its message; surface it with the old exit code.
        Err(e) if e.to_string().contains("deleted from patch chain") => {
            eprintln!("{e} — the client does not load this path (see decision 0246)");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
