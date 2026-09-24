//! Each named DBC's header and first rows' raw dwords, to check a column offset against the file.
fn main() -> anyhow::Result<()> {
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    for name in std::env::args().skip(1) {
        let path = format!("DBFilesClient\\{name}.dbc");
        let Ok(bytes) = chain.read_file(&path) else {
            println!("{name}: NOT IN CHAIN");
            continue;
        };
        let g = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let (rows, fields, rec) = (g(4), g(8), g(12));
        println!(
            "{name}: {rows} rows x {fields} fields, record {rec} bytes (0x{rec:x}), stringblock {}",
            g(16)
        );
        for r in 0..rows.min(4) as usize {
            let base = 20 + r * rec as usize;
            let mut out = String::new();
            for f in 0..fields as usize {
                let v = g(base + f * 4);
                let fl = f32::from_le_bytes(v.to_le_bytes());
                out += &format!(
                    " [{:#04x}]{}",
                    f * 4,
                    if (0.0001..100000.0).contains(&fl.abs()) && v > 0x1000 {
                        format!("{fl:.3}f")
                    } else {
                        format!("{v}")
                    }
                );
            }
            println!("  row{r}:{out}");
        }
    }
    Ok(())
}
