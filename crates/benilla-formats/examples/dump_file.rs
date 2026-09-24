//! Any file from the patch chain to stdout, such as a FrameXML source or a DBC.
//! `cargo run -p benilla-formats --example dump_file -- 'Interface\FrameXML\TradeSkillFrame.lua'`
//! Output is Blizzard data: never commit it.

use std::io::Write;

fn main() -> anyhow::Result<()> {
    let virt = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: dump_file <virtual\\mpq\\path>"))?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let bytes = chain.read_file(&virt)?;
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}
