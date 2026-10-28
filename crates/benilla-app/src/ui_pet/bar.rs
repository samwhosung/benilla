//! The pet action bar's paint: the ten packed words of the last `SMSG_PET_SPELLS`, which the
//! server owns whole, as the getters read each one at the call ([`super::live`]).

use benilla_protocol::messages::{
    PetActionEntry, PetSpells, PET_COMMAND_ATTACK, PET_COMMAND_DISMISS, PET_COMMAND_FOLLOW,
    PET_COMMAND_STAY, PET_REACT_AGGRESSIVE, PET_REACT_DEFENSIVE, PET_REACT_PASSIVE,
};
use benilla_ui::script::PetActionView;

use crate::net::ObjectStore;

use super::PetBar;

/// The world entry's `SetPetGuid(0, 0)` (`0x4bc6f0` → `0x4bc7e0` at `0x4bc6fd`): the guid and the
/// expiry cleared (`0x4bc822`, `0x4bc83d`) and the attack latch lowered (`0x4bc8ce`), so the
/// getters read no bar and no pet UI, as `HasPetUI` resolves the pet from the cleared guid
/// (`0x4be67e`-`0x4be690`); the VM's copy goes too. Whether the guid changed, for `UNIT_PET`
/// (`0x4bc84f`); the caller signals it and `PET_BAR_UPDATE` (`0x4bc90c`).
///
/// The rest stays as the reference leaves it: the slots and the mode word, whose every reader
/// tests the guid first (`GetPetActionInfo` `0x4bdc9d`, `GetPetActionCooldown` `0x4bdff2`,
/// `IsPetAttackActive` `0x4be123`, `PickupPetAction` `0x4be249`, `CastPetAction` `0x4be414`, the
/// tooltip `0x532809`, the drop `0x4bce12`) or passes the bar's gate, which resolves the pet from
/// it (`0x4bcf70`, `0x4bd034`); the pet book, which `HasPetSpells` counts with no guid
/// (`0x4b4416`); and the pet's cooldown list.
pub(crate) fn drop_bar(
    script: &mut benilla_ui::script::UiScript,
    bar: &mut PetBar,
    latch: &mut super::PetAttackLatch,
) -> bool {
    let changed = bar.has_bar();
    bar.spells.pet_guid = 0;
    bar.expires = None;
    // `0x4bc8ce` runs on a change only, and the latch is only ever up under a guid.
    if changed {
        latch.0 = None;
    }
    script.drop_pet_bar();
    changed
}

/// The world-enter cascade's pet reset (`0x4909c9` → `0x4bc6f0`) in its place
/// ([`crate::ui_world_enter`]): `SetPetGuid(0, 0)` as an engine turn on the bar and the VM
/// ([`drop_bar`]), its signals with the host lent, then `CMSG_REQUEST_PET_INFO` (`0x4bc738`) when
/// our charm, else our summon, is set (`0x4bc721`-`0x4bc736`). Its trainer refresh (`0x4bc872`)
/// never runs here: the cascade zeroes the trainer cell before it (`0x4909ab` → `0x4d73e0`).
#[allow(clippy::type_complexity)]
pub(crate) fn reset_pet_on_world_enter(
    mut vm: crate::script_calls::VmWith<(
        Option<bevy::prelude::ResMut<'static, PetBar>>,
        Option<bevy::prelude::ResMut<'static, super::PetAttackLatch>>,
    )>,
    me: bevy::prelude::Query<&ObjectStore, bevy::prelude::With<crate::net::SelfPlayer>>,
    commands: bevy::prelude::Res<crate::net::NetCommands>,
) {
    let dropped = vm.turn(|script, (bar, latch)| {
        let script = script?;
        Some(match (bar, latch) {
            (Some(mut bar), Some(mut latch)) => drop_bar(script, &mut bar, &mut latch),
            _ => script.drop_pet_bar(),
        })
    });
    if let Some(dropped) = dropped {
        vm.run(|script| {
            if let Some(script) = script {
                script.fire_pet_bar_dropped(dropped);
            }
        });
    }
    if me
        .iter()
        .next()
        .is_some_and(|s| s.0.unit_pet_guid().is_some())
    {
        let _ = commands.0.send(crate::net::ClientCommand::RequestPetInfo);
    }
}

/// A command token's `(name, texture)` pair, both global names (`PetActionBarFrame.lua:98-104`):
/// keys from `GlobalStrings.lua:3029-3032`, textures from `PetActionBarFrame.lua:6-12`.
pub(super) fn command_token(action: u32) -> Option<(&'static str, &'static str)> {
    Some(match action {
        PET_COMMAND_STAY => ("PET_ACTION_WAIT", "PET_WAIT_TEXTURE"),
        PET_COMMAND_FOLLOW => ("PET_ACTION_FOLLOW", "PET_FOLLOW_TEXTURE"),
        PET_COMMAND_ATTACK => ("PET_ACTION_ATTACK", "PET_ATTACK_TEXTURE"),
        PET_COMMAND_DISMISS => ("PET_ACTION_DISMISS", "PET_DISMISS_TEXTURE"),
        _ => return None,
    })
}

/// A reaction token's pair: the `PET_MODE_*` keys (`GlobalStrings.lua:3045-3047`), naming the
/// pet's state, not the menu's imperative `PET_AGGRESSIVE` family, though enUS reads them alike.
pub(super) fn reaction_token(action: u32) -> Option<(&'static str, &'static str)> {
    Some(match action {
        PET_REACT_PASSIVE => ("PET_MODE_PASSIVE", "PET_PASSIVE_TEXTURE"),
        PET_REACT_DEFENSIVE => ("PET_MODE_DEFENSIVE", "PET_DEFENSIVE_TEXTURE"),
        PET_REACT_AGGRESSIVE => ("PET_MODE_AGGRESSIVE", "PET_AGGRESSIVE_TEXTURE"),
        _ => return None,
    })
}

/// One packed slot word as the bar draws it. `showing_active` is [`active_aura_press`]'s answer,
/// the one call (`0x4bcea0`) that both `GetPetActionInfo` (`0x4bdd2f`) and `CastPetAction`
/// (`0x4bd24a`) make, so the button showing active art is the one whose press cancels.
pub(super) fn slot_view(
    entry: PetActionEntry,
    bar: &PetSpells,
    spell: Option<&benilla_formats::SpellDisplay>,
    cooldown: Option<(i64, u32, bool)>,
    pet_attacking: bool,
    showing_active: bool,
) -> PetActionView {
    PetActionView {
        // Every slot carries its raw word, empty ones too: the drag's relocation candidate is
        // vmangos's unused slot, `ACT_DISABLED` with spell 0 (type 1, low 16 bits zero).
        packed: entry.packed,
        passive: spell.is_some_and(|s| s.passive),
        ..slot_paint(entry, bar, spell, cooldown, pet_attacking, showing_active)
    }
}

/// [`slot_view`]'s painted half: everything but the raw `packed` and `passive`.
pub(super) fn slot_paint(
    entry: PetActionEntry,
    bar: &PetSpells,
    spell: Option<&benilla_formats::SpellDisplay>,
    cooldown: Option<(i64, u32, bool)>,
    pet_attacking: bool,
    showing_active: bool,
) -> PetActionView {
    let kind = entry.kind();
    let action = entry.action();

    if let Some((name, texture)) = (kind == benilla_protocol::messages::PET_ACT_COMMAND)
        .then(|| command_token(action))
        .flatten()
    {
        // Lit when the unmasked `state >> 8` equals the action, so a disabled bar lights none, or
        // for Attack alone on the attack latch (`0x4bdf01`-`0x4bdf22`). `attack_active` is
        // `IsPetAttackActive`, that latch on this slot (`0x4be138`-`0x4be153`).
        let attacking = pet_attacking && action == PET_COMMAND_ATTACK;
        return PetActionView {
            name: Some(name.to_string()),
            texture: Some(texture.to_string()),
            is_token: true,
            active: bar.command_state() == action || attacking,
            attack_active: attacking,
            ..Default::default()
        };
    }

    if let Some((name, texture)) = (kind == benilla_protocol::messages::PET_ACT_REACTION)
        .then(|| reaction_token(action))
        .flatten()
    {
        // A disabled bar reads as Passive (`0x4bde3c`).
        let showing = if bar.bar_disabled() {
            benilla_protocol::messages::PET_REACT_PASSIVE
        } else {
            bar.react_state()
        };
        return PetActionView {
            name: Some(name.to_string()),
            texture: Some(texture.to_string()),
            is_token: true,
            active: showing == action,
            ..Default::default()
        };
    }

    // A zero word is empty (the client tests the dword); vmangos's unused slots are spell 0, which
    // has no record and takes the next exit, as in the client.
    if !entry.is_spell() || entry.is_empty() {
        return PetActionView::default();
    }
    let Some(spell) = spell else {
        // No `Spell.dbc` record, or no catalog: the reference returns nil and the button hides.
        return PetActionView::default();
    };
    PetActionView {
        name: Some(spell.name.clone()),
        subtext: spell.rank.clone(),
        // A running spell draws its `ActiveIconID` (`0x4bdd2f`/`0x4bdd38`/`0x4bdd77`), with no
        // fallback: the reference pushes nil when that icon does not resolve (`0x4bdd50`).
        texture: if showing_active {
            spell.active_icon.clone()
        } else {
            spell.icon.clone()
        },
        is_token: false,
        spell_id: Some(action),
        // Nil on every spell path (`0x4bdd5e`): `isActive` is a token's, `autoCast*` a spell's
        // (`0x4bdc50`).
        active: false,
        // Bits 31/30, not the type (`0x4bdd65`/`0x4bdda4`), gated on the record checked above.
        autocast_allowed: entry.autocast_allowed(),
        autocast_enabled: entry.autocast_on(),
        attack_active: false,
        cooldown,
        ..Default::default()
    }
}

/// The spell id when the slot shows active: `0x4bcea0`, the pet-side twin of `0x4e55f0`
/// ([`crate::ui_action::toggle::active_action_toggle`]). It tests `ActiveIconID != 0` first
/// (`0x4bcefd`), so a press on a spell without one re-casts rather than cancels.
pub(crate) fn active_aura_press(
    entry: PetActionEntry,
    pet: Option<&ObjectStore>,
    spell: Option<&benilla_formats::SpellDisplay>,
) -> Option<u32> {
    if !entry.is_spell() || entry.is_empty() {
        return None;
    }
    let spell_id = entry.action();
    crate::ui_action::toggle::active_action_toggle(spell_id, spell?, pet?).then_some(spell_id)
}
