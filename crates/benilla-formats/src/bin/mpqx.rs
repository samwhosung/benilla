//! Extraction helper, not shipped: `mpqx <data-dir> <virtual-path> <out>`, read through the
//! runtime's patch chain ([`benilla_formats::Chain`]). It exits 2 on a path the top archive
//! delete-marks, which the client does not load.

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
        // `Chain::read` names a delete-marked path in its error message.
        Err(e) if e.to_string().contains("deleted from patch chain") => {
            eprintln!("{e} — the client does not load this path");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
