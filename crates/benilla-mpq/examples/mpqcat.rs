//! `mpqcat`: write one file out of an MPQ to stdout, to read the client's own data (FrameXML,
//! GlobalStrings, a BLP header).
//!
//! ```text
//! cargo run -q -p benilla-mpq --example mpqcat -- <archive.MPQ> 'Interface\FrameXML\UIParent.lua'
//! ```
//!
//! Names are the client's backslash paths. A miss is an error, not empty output: the same file
//! often lives in several archives (patch over interface over base), and "not here" must read
//! apart from "here but empty".
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(archive), Some(name)) = (args.next(), args.next()) else {
        eprintln!("usage: mpqcat <archive.MPQ> <file-in-archive>");
        std::process::exit(2);
    };
    let bytes = benilla_mpq::Archive::open(&archive)?.read_file(&name)?;
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}
