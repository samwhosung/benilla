//! The targeting columns of `Spell.dbc` for the given spell ids: `Targets`,
//! `EffectImplicitTargetA[0]`, the first effect and the equipped-item gate columns.
fn main() -> anyhow::Result<()> {
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let catalog = benilla_formats::load_spell_catalog(&mut chain)?;
    for arg in std::env::args().skip(1) {
        let id: u32 = arg.parse()?;
        let Some(d) = catalog.get(id) else {
            println!("{id}: no row");
            continue;
        };
        println!(
            "{id} {:?}\n  Targets={:#06x} implicitA1={} effects[0] = {}\n  equippedClass={} subclassMask={:#x} invTypeMask={:#x}",
            d.name,
            d.targets,
            d.implicit_target_a1,
            d.effects[0],
            d.equipped_item_class,
            d.equipped_item_subclass_mask,
            d.equipped_item_inventory_type_mask,
        );
    }
    Ok(())
}
