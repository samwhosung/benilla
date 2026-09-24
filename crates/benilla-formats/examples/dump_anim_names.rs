//! `AnimationData.dbc` rows (id, name, weapon flags, fallback), all of them without ids.
//! `cargo run -p benilla-formats --example dump_anim_names <Data-dir> [id ...]`

fn main() {
    let mut args = std::env::args().skip(1);
    let data = args
        .next()
        .expect("usage: dump_anim_names <Data-dir> [id ...]");
    let ids: Vec<u16> = args.map(|a| a.parse().expect("id u16")).collect();
    let mut chain = benilla_formats::Chain::open(std::path::Path::new(&data)).expect("chain");
    let cat = benilla_formats::load_anim_data_catalog(&mut chain).expect("catalog");
    for id in 0..u16::try_from(cat.len() + 64).unwrap() {
        if !ids.is_empty() && !ids.contains(&id) {
            continue;
        }
        if let Some(name) = cat.name(id) {
            println!(
                "{id:3} {name:24} wflags {:#04x} fallback {:?}",
                cat.weapon_flags(id),
                cat.fallback(id)
            );
        }
    }
}
