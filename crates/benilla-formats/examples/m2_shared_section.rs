//! M2 batches that share a skin section, and so the same triangles, with another batch. The
//! reference draws both from one vertex array under depth-write and LEQUAL, so the later one wins
//! the tie exactly (`0x70b3f6`, `0x70c190`). We match that only when both take the same vertex
//! path: a consolidator (`static_gx`, `terrain_stream::merge`) bakes positions on the CPU, which
//! differ from the GPU's in the last ULPs, so a section it splits z-fights (a `SPLIT` line).
//! Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example m2_shared_section -- [path-substring]`

use std::io::Cursor;

/// What every consolidator excludes on: a `true` batch stays on the entity path whatever its
/// siblings do.
fn refusable(model: &benilla_m2::M2Model, batch: &benilla_m2::SkinBatch) -> bool {
    let mat = model.materials.get(batch.material_index as usize);
    let flags = mat.map_or(0, |m| m.flags.bits());
    let blend = mat.map_or(0, |m| m.blend_mode.bits());
    model.stage_is_env_mapped(batch, 0)
        || flags & 0x10 != 0
        || flags & 0x08 != 0
        || matches!(blend, 3 | 4)
}

fn main() -> anyhow::Result<()> {
    let pat = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let names: Vec<String> = chain
        .list()?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".m2") && l.contains(&pat)
        })
        .collect();
    let (mut models, mut with_shared, mut with_split) = (0usize, 0usize, 0usize);
    let (mut batches_total, mut batches_shared, mut batches_split) = (0usize, 0usize, 0usize);
    for name in &names {
        let Ok(bytes) = chain.read_file(name) else {
            continue;
        };
        let Ok(m2) = benilla_m2::parse_m2(&mut Cursor::new(bytes.as_slice())) else {
            continue;
        };
        let model = m2.model();
        let Ok(skin) = model.parse_embedded_skin(&bytes, 0) else {
            continue;
        };
        models += 1;
        batches_total += skin.batches().len();
        // section index → (batch count, refusable count)
        let mut per_section: std::collections::BTreeMap<u16, (usize, usize)> = Default::default();
        for b in skin.batches() {
            let e = per_section.entry(b.skin_section_index).or_default();
            e.0 += 1;
            e.1 += usize::from(refusable(model, b));
        }
        let shared: Vec<_> = per_section.iter().filter(|(_, v)| v.0 > 1).collect();
        if shared.is_empty() {
            continue;
        }
        with_shared += 1;
        let mut split = false;
        for (sec, (n, r)) in &shared {
            batches_shared += n;
            // Disagreement is the defect: some of this section's batches divert, some cannot.
            if *r > 0 && *r < *n {
                split = true;
                batches_split += n;
                println!("SPLIT  {name}  section {sec}: {n} batches, {r} refusable");
            }
        }
        if split {
            with_split += 1;
        }
    }
    println!(
        "\n{models} models parsed, {batches_total} batches\n\
         {with_shared} models have a multi-batch section ({batches_shared} batches in one)\n\
         {with_split} models SPLIT one ({batches_split} batches) — coplanar siblings that land in \
         different lanes"
    );
    Ok(())
}
