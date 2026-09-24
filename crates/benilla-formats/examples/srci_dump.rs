//! `SkillRaceClassInfo.dbc` rows admitting a race and class, flags in hex: `GetSkillLineInfo` reads
//! its 0x400 single-rank cap and 0x20 unlearnable bit here.
//! `cargo run -p benilla-formats --example srci_dump -- <race> <class>`
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let race: u32 = args.next().unwrap_or_else(|| "4".into()).parse()?;
    let class: u32 = args.next().unwrap_or_else(|| "3".into()).parse()?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;

    let lines = chain.read_file("DBFilesClient\\SkillLine.dbc")?;
    let name_of = |id: u32| -> String {
        let g = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let (rows, _fields, rec) = (g(&lines, 4), g(&lines, 8), g(&lines, 12) as usize);
        let sb = 20 + rows as usize * rec;
        for r in 0..rows as usize {
            let base = 20 + r * rec;
            if g(&lines, base) == id {
                let off = sb + g(&lines, base + 3 * 4) as usize;
                let end = lines[off..].iter().position(|&c| c == 0).unwrap_or(0) + off;
                return format!(
                    "{} (cat {})",
                    String::from_utf8_lossy(&lines[off..end]),
                    g(&lines, base + 4)
                );
            }
        }
        "?".into()
    };

    let bytes = chain.read_file("DBFilesClient\\SkillRaceClassInfo.dbc")?;
    let g = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let (rows, _fields, rec) = (g(4), g(8), g(12) as usize);
    let (rmask, cmask) = (
        1u32.checked_shl(race.wrapping_sub(1)).unwrap_or(0),
        1u32.checked_shl(class.wrapping_sub(1)).unwrap_or(0),
    );
    println!("race {race} (mask {rmask:#x}) class {class} (mask {cmask:#x})");
    let mut seen: Vec<u32> = Vec::new();
    for r in 0..rows as usize {
        let base = 20 + r * rec;
        let (skill, race_mask, class_mask, flags, req_level, tier) = (
            g(base + 4),
            g(base + 8),
            g(base + 12),
            g(base + 16),
            g(base + 20),
            g(base + 24),
        );
        // Race or class 0 dumps every row, unfiltered, duplicates kept.
        let all = race == 0 || class == 0;
        if !all && (race_mask & rmask == 0 || class_mask & cmask == 0 || seen.contains(&skill)) {
            continue;
        }
        seen.push(skill);
        println!(
            "skill {skill:>4} flags {flags:#07x} (0x400={}) reqLevel {req_level} tier {tier}  {}",
            flags & 0x400 != 0,
            name_of(skill)
        );
    }
    Ok(())
}
