//! `ChrClasses.dbc`, loaded once for its unrelated readers: the pet book (field 4, pet name token),
//! `UnitHasRelicSlot` (field 16) and `crate::spell::mods` (field 15, spell family). Absent when the
//! load fails; each reader then takes the reference's degraded answer: `"PET"`, no relic slot,
//! family 0.

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::ChrClasses;

/// The parsed table; [`benilla_formats::ChrClasses`] documents the columns.
#[derive(Resource)]
pub(crate) struct ChrClassTable(pub(crate) ChrClasses);

pub(crate) struct ChrClassesPlugin;

impl Plugin for ChrClassesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_chr_classes.after(AssetSet::Open));
    }
}

fn load_chr_classes(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_chr_classes(&mut chain)
    };
    match loaded {
        Ok(table) => commands.insert_resource(ChrClassTable(table)),
        Err(e) => warn!(
            "chr_classes: ChrClasses.dbc failed to load — every pet book tab reads the client's \
             own \"PET\" fallback, so a warlock's says Pet rather than Demon, no class reads \
             as having a relic slot, and no talent spell modifier can apply (the gate has no \
             class family to match): {e:#}"
        ),
    }
}
