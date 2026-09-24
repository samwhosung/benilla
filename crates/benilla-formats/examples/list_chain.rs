//! Every file in the patch chain whose path contains a substring, case-insensitive, optionally of
//! one extension. `cargo run -p benilla-formats --example list_chain -- maraudon [m2]`
//! Output is Blizzard data: never commit it.

fn main() -> anyhow::Result<()> {
    let pat = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: list_chain <substring> [ext]"))?
        .to_lowercase();
    let ext = std::env::args()
        .nth(2)
        .map(|e| format!(".{}", e.to_lowercase()));
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let chain = benilla_formats::open_chain(&data)?;
    for e in chain.list()? {
        let lower = e.name.to_lowercase();
        if lower.contains(&pat) && ext.as_ref().is_none_or(|x| lower.ends_with(x)) {
            println!("{:>9}  {}", e.size, e.name);
        }
    }
    Ok(())
}
