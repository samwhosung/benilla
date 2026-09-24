//! The interrupt-flag and attribute columns of `Spell.dbc` for the given spell ids.
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
            "{id} {:?}\n  InterruptFlags={:#x} (movement bit 0x1 = {})\n  AuraInterruptFlags={:#x} ChannelInterruptFlags={:#x}\n  Attributes={:#x} AttributesEx={:#x} AttributesEx2={:#x}",
            d.name,
            d.interrupt_flags,
            d.interrupt_flags & 0x1 != 0,
            d.aura_interrupt_flags,
            d.channel_interrupt_flags,
            d.attributes,
            d.attributes_ex,
            d.attributes_ex2,
        );
    }
    Ok(())
}
