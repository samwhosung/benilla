//! Melee combat audio. `SMSG_ATTACKERSTATEUPDATE` arrives as a [`SwingMessage`], then the attack
//! clip's M2 events fire mid-swing through [`AnimSoundEvent`] and [`SwingImpact`].
//!
//! - Exertion, the attacker's vocal: packet-driven at swing start (`0x6246a0` → `0x624786`),
//!   only when victimState is nonzero (`0x62476a`), crit bit as class, rolled unless a crit.
//! - `$CSS`, the whoosh: victimState alone picks it ([`whiffed`], `0x624ca0`), the miss whoosh by
//!   handedness or `WeaponSwingSounds2` by weapon weight ([`swing_weight`]).
//! - Contact, at the `$AH0-3`/`$CAH` crossing ([`SwingImpact`]): first the attacker's
//!   weapon-sound block (`0x6247d0`: the natural-weapon `CustomAttack[n]` column, else the
//!   landed impact for the victim's slot), then the victim dispatch (`0x624530`: the parry or
//!   block clang, the deflect and absorb stubs, the injury vocal). `$CAH` drives the victim's
//!   injury (`0x624865`), never the attacker's exertion.
//!
//! Metal vs wood is `Material.dbc`'s `Flags & 1` throughout (`0x457e80`). Untraced: whether the
//! generic impact also plays under a clang (`0x624936`) and whether the natural-weapon column
//! plays on a whiff; benilla plays neither. A `text_only` flush plays nothing. `$CPP` and `$CST`
//! are not audio.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use benilla_formats::{impact_slot, WeaponImpactCatalog};

use crate::creature_anim::{AnimSoundEvent, SwingImpact, SwingMessage, Wielded};
use crate::net::{Embodied, NetEntity};
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_protocol::EntityKind;
use benilla_world::schedule::WorldStage;

use super::creature::CreatureVoices;
use super::kit::{
    bark_chance_pass, object_sound_playing, play_kit_ext, Bus, KitRef, PlayExtras, SoundCategory,
    SoundKits, Volume, EXERTION_CHANCE_CREATURE, EXERTION_CHANCE_PLAYER, INJURY_CHANCE_CREATURE,
    INJURY_CHANCE_PLAYER,
};
use super::{AudioListener, SoundConfig, SoundOutput};

// vmangos `HitInfo` bits (UnitDefines.h, 1.12 wire).
const HITINFO_MISS: u32 = 0x10;
const HITINFO_CRITICAL: u32 = 0x80;
const HITINFO_CRUSHING: u32 = 0x8000;
// vmangos `VictimState`.
const VICTIM_DODGE: u32 = 2;
const VICTIM_PARRY: u32 = 3;
const VICTIM_BLOCK: u32 = 5;
const VICTIM_EVADE: u32 = 6;
const VICTIM_IMMUNE: u32 = 7;
const VICTIM_DEFLECT: u32 = 8;

/// `HITINFO_LEFTSWING`, the offhand swing; the reference picks the hand by it
/// (`0x624c36`: `(hitInfo >> 2) & 1`).
const HITINFO_LEFTSWING: u32 = 0x4;

/// The `(DONOTRENAME)Combat Miss 1H/2H` whooshes, cached by name at startup (`0x4575b0`).
const COMBAT_MISS_1H: u32 = 7080;
const COMBAT_MISS_2H: u32 = 7081;

/// The victim dispatch's fixed stub kits (`0x624530`), from the same startup name cache
/// (`0x835e8c`/`0x835eac`), played at the victim two yards up on bus 0 at volume 1.0
/// (`0x458870`): deflect (victimState 8, `0x6245f5`) plays `(DONOTRENAME)ShieldWoodImpact`;
/// absorb, resist or immune (`0x62460f`, `0x624613`) play `(DONOTRENAME)AbsorbGetHit`.
const DEFLECT_KIT: u32 = 3262;
const ABSORB_KIT: u32 = 3334;

/// `HITINFO_ABSORB | HITINFO_RESIST`, the reference's `test al, 0x60`: [`ABSORB_KIT`] instead of
/// the wound vocal.
const HITINFO_ABSORB_OR_RESIST: u32 = 0x60;

/// The stub emitters' lift (`0x457f4a`/`0x45863a`: `fadd [0x801628]`), as the armor foley's; the
/// generic weapon impact passes its position through unlifted.
const STUB_HEIGHT: f32 = 2.0;

/// `ItemClass` 2, a weapon (`0x6238b4`); any other held item swings in silence.
const ITEM_CLASS_WEAPON: u32 = 2;

/// Weapon subclasses swung two-handed, for the 2H miss whoosh.
const TWO_HANDED: [u32; 6] = [1, 5, 6, 8, 10, 17];
/// The fist subclass, the row a weaponless swing uses.
const UNARMED_SUBCLASS: u32 = 13;

/// `WeaponSwingSounds2.dbc`, the connecting swing's kit by `(weight, critical)`.
#[derive(Resource)]
pub(crate) struct WeaponSwings(pub(crate) benilla_formats::WeaponSwingCatalog);

fn load_weapon_swings(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_weapon_swing_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => commands.insert_resource(WeaponSwings(cat)),
        Err(e) => warn!("sound: weapon swing sounds failed to load: {e:#}"),
    }
}

#[derive(Resource)]
pub(crate) struct WeaponImpacts(pub(crate) WeaponImpactCatalog);

fn load_weapon_impacts(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_weapon_impact_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} weapon impact rows", cat.len());
            commands.insert_resource(WeaponImpacts(cat));
        }
        Err(e) => warn!("sound: weapon impacts failed to load: {e:#}"),
    }
}

/// The latest swing per attacker, read when its `$CSS` event fires in a later frame.
#[derive(Default)]
struct LastSwing(EntityHashMap<SwingMessage>);

/// The attacker's swinging weapon as `(subclass, metal)`, unarmed for an empty hand. `metal` is
/// the item's `Material` with `Flags & 1` (`0x457e80` → `0x5d9a50`), so leather, cloth and the
/// unknown id 0 are non-metal, as is an empty hand (`0x457e8d`).
fn swing_weapon(
    wielded: Option<&Wielded>,
    offhand: bool,
    materials: Option<&benilla_formats::MaterialCatalog>,
) -> (u32, bool) {
    // `0x625400`/`0x625460` pass `visFlag = 0`: a disarmed hand swings as unarmed.
    let hand = wielded.and_then(|w| {
        if offhand {
            w.armed_off()
        } else {
            w.armed_main()
        }
    });
    match hand {
        // class 2 = weapon; anything else in hand (held misc) swings as unarmed.
        Some((2, subclass)) => {
            let material = u32::from(wielded.map_or(0, |w| {
                w.materials[usize::from(offhand).min(w.materials.len() - 1)]
            }));
            (
                u32::from(subclass),
                materials.is_some_and(|m| m.is_metal(material)),
            )
        }
        _ => (UNARMED_SUBCLASS, false),
    }
}

/// A creature victim's `WeaponImpactSounds` slot (`[vt+0x90]`, `0x6238f0`): its
/// `CreatureSoundData` column, `>= 4` refused, remapped by `{0, 8, 7, 9}` (`0x80db94`) to flesh,
/// stone, wood, ethereal. A player victim instead presents its chest armor (`0x62fb70`, at the
/// call site), the only route to the chain and plate slots.
fn creature_impact_slot(impact_type: u32) -> usize {
    match impact_type {
        1 => impact_slot::STONE,
        2 => impact_slot::WOOD,
        3 => impact_slot::ETHEREAL,
        _ => impact_slot::FLESH,
    }
}

/// The `$CSS` split (`0x624ca0`), on victimState alone: unaffected, dodge and evade take the
/// miss whoosh, every other outcome the connecting swing, never both (`0x624ba4`). Not
/// [`no_contact`]: this reads no hit flags, and immune and deflect connect.
fn whiffed(victim_state: u32) -> bool {
    matches!(victim_state, 0 | VICTIM_DODGE | VICTIM_EVADE)
}

/// The swinging weapon's weight (`0x623870`): `ItemSubClass.dbc`'s `WeaponSwingSize` (field 9)
/// for a weapon, Light for an empty hand (`0x623892`), `None` for a held non-weapon
/// (`0x6238b7`), which plays nothing.
fn swing_weight(
    wielded: Option<&Wielded>,
    offhand: bool,
    sub_classes: &benilla_formats::ItemSubClassCatalog,
) -> Option<u32> {
    // `visFlag = 0`: a disarmed hand takes the empty-hand leg.
    match wielded.and_then(|w| {
        if offhand {
            w.armed_off()
        } else {
            w.armed_main()
        }
    }) {
        None => Some(0),
        Some((class, subclass)) if u32::from(class) == ITEM_CLASS_WEAPON => {
            sub_classes.weapon_swing_size(ITEM_CLASS_WEAPON, u32::from(subclass))
        }
        Some(_) => None,
    }
}

/// Nothing for the weapon to strike, so no weapon impact or clang; immune and deflect play their
/// stub kits instead (`0x457f20`/`0x458610`).
fn no_contact(swing: &SwingMessage) -> bool {
    swing.hit_info & HITINFO_MISS != 0
        || matches!(
            swing.victim_state,
            VICTIM_DODGE | VICTIM_EVADE | VICTIM_IMMUNE | VICTIM_DEFLECT
        )
}

/// Parry or block: the sound is the victim's clang ([`defense_clang`]), not the weapon impact.
fn defended(victim_state: u32) -> bool {
    matches!(victim_state, VICTIM_PARRY | VICTIM_BLOCK)
}

/// The victim's defending item as `(class, material)` (`0x625400(sel)`): a parry takes a
/// mainhand weapon, else the offhand; a block takes the offhand; it must be class 2 or 4. `None`
/// plays no clang (`0x623690`).
fn defending_item(wielded: Option<&Wielded>, block: bool) -> Option<(u8, u8)> {
    let w = wielded?;
    // `visFlag = 0`: a disarmed mainhand falls through to the offhand.
    if !block {
        if let Some((2, _)) = w.armed_main() {
            return Some((2, w.materials[0]));
        }
    }
    let (class, _) = w.armed_off()?;
    matches!(class, 2 | 4).then_some((class, w.materials[1]))
}

/// The clang (`0x623640` → `0x457dc0`): the attacker's weapon row, slot by the defending item's
/// class, not the victimState (`0x457de6`/`0x457dfd`): class 2 the parry pair, class 4 the
/// shield pair, its `Material` picking metal or wood. Not crit-tiered.
fn defense_clang(
    row: &benilla_formats::WeaponImpactRow,
    item: Option<(u8, u8)>,
    materials: Option<&benilla_formats::MaterialCatalog>,
) -> u32 {
    let metal = |m: u8| materials.is_some_and(|c| c.is_metal(u32::from(m)));
    let slot = match item {
        Some((2, m)) if metal(m) => impact_slot::PARRY_METAL,
        Some((2, _)) => impact_slot::PARRY_WOOD,
        Some((4, m)) if metal(m) => impact_slot::SHIELD_METAL,
        Some((4, _)) => impact_slot::SHIELD_WOOD,
        _ => impact_slot::FLESH,
    };
    row.impact[slot]
}

/// The generic weapon impact for a landed hit (`0x6247d0`), crit-tiered.
fn landed_impact(row: &benilla_formats::WeaponImpactRow, slot: usize, crit: bool) -> u32 {
    if crit {
        row.crit[slot]
    } else {
        row.impact[slot]
    }
}

/// A unit as the combat sounds see it; the store is for a player victim's chest armor.
type CombatUnit = (
    &'static Transform,
    Option<&'static Wielded>,
    &'static NetEntity,
    Has<Embodied>,
    Option<&'static crate::net::ObjectStore>,
);

/// The attachment a whiffed swing's whoosh plays at (`0x624bdd`: `0x712cb0(1)`).
const MISS_ATTACH: u16 = 1;

/// The DBC tables the melee sounds read, each optional: a missing one silences its own branch.
#[derive(bevy::ecs::system::SystemParam)]
struct MeleeTables<'w> {
    impacts: Option<Res<'w, WeaponImpacts>>,
    swing_sounds: Option<Res<'w, WeaponSwings>>,
    materials: Option<Res<'w, super::Materials>>,
    /// Owned by the tooltip; the whoosh reads the same rows' weight column.
    sub_classes: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    voices: Option<Res<'w, CreatureVoices>>,
}

fn combat_sounds(
    mut swings: MessageReader<SwingMessage>,
    mut contacts: MessageReader<SwingImpact>,
    mut events: MessageReader<AnimSoundEvent>,
    mut last: Local<LastSwing>,
    units: Query<CombatUnit>,
    tables: MeleeTables,
    attach: crate::entities::AttachPoints,
    objects: crate::net::Objects,
    mut items: Option<ResMut<crate::items::Items>>,
    net_commands: Res<crate::net::NetCommands>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    // Exertion fires from the packet, at swing start (`0x6246a0` → `0x624786`).
    let mut exertions: Vec<(Entity, bool)> = Vec::new();
    for s in swings.read() {
        last.0.insert(s.attacker, *s);
        // `0x62476a`: gated on victimState alone, with no victim-health test.
        if s.victim_state != 0 {
            exertions.push((s.attacker, s.hit_info & HITINFO_CRITICAL != 0));
        }
    }
    // Sweep despawned attackers so a long session does not accumulate dead keys.
    if last.0.len() > 128 {
        last.0.retain(|e, _| units.contains(*e));
    }
    if events.is_empty() && contacts.is_empty() && exertions.is_empty() {
        return;
    }
    let MeleeTables {
        impacts,
        swing_sounds,
        materials,
        sub_classes,
        voices,
    } = tables;
    let (Some(impacts), Some(voices), Some(mut kits), Some(assets)) =
        (impacts, voices, kits, assets)
    else {
        return;
    };
    let listener = listener.pos;
    // Without `Material.dbc` every weapon is non-metal and every victim flesh, the reference's
    // answers for an unknown material.
    let mats = materials.as_deref().map(|m| &m.0);
    // Every play carries its voice bus; a kit refused at the bus cap is a silent success.
    let play = |kits: &mut SoundKits,
                out: &mut SoundOutput,
                kit: u32,
                pos: Vec3,
                extras: PlayExtras,
                what: &str| {
        if kit == 0 {
            return;
        }
        if let Err(e) = play_kit_ext(
            kits,
            &assets,
            out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(pos),
            SoundCategory::Sfx,
            extras,
        ) {
            warn!("combat {what} (kit {kit}): {e:#}");
        }
    };

    // `force = 0` (`0x62477e`), so the class roll applies: class 0 is 70 for a creature and 35
    // for a player, class 1 (a crit) 100.
    for (attacker, crit) in exertions {
        let Ok((tr, _, net, _, _)) = units.get(attacker) else {
            continue;
        };
        // The `AISOUNDDESC` gate on the attacker: exertion is classes 0/1.
        if net.kind != EntityKind::Player && object_sound_playing(&out, attacker) {
            continue;
        }
        if !crit {
            let threshold = if net.kind == EntityKind::Player {
                EXERTION_CHANCE_PLAYER
            } else {
                EXERTION_CHANCE_CREATURE
            };
            if !bark_chance_pass(threshold, kits.roll()) {
                continue;
            }
        }
        let vocal = net
            .display_id
            .and_then(|d| voices.0.for_display(d))
            .map(|v| v.exertion[usize::from(crit)])
            .unwrap_or(0);
        play(
            &mut kits,
            &mut out,
            vocal,
            tr.translation,
            PlayExtras {
                bus: Bus::EXERTION,
                ..default()
            },
            "exertion",
        );
    }

    // `$CSS`: the miss whoosh on bus 0, or `WeaponSwingSounds2` on bus 6 (cap 2).
    for ev in events.read() {
        if ev.ident != *b"$CSS" {
            continue;
        }
        let Some(swing) = last.0.get(&ev.entity) else {
            continue; // an attack anim without a tracked swing (e.g. spawned mid-fight)
        };
        let Ok((attacker_tr, wielded, _, _, _)) = units.get(ev.entity) else {
            continue;
        };
        let offhand = swing.hit_info & HITINFO_LEFTSWING != 0;
        if whiffed(swing.victim_state) {
            let (subclass, _) = swing_weapon(wielded, offhand, mats);
            let kit = if TWO_HANDED.contains(&subclass) {
                COMBAT_MISS_2H
            } else {
                COMBAT_MISS_1H
            };
            // Attachment 1, not the event's point: a whiff plays from the hand (`0x624baa`).
            let at = attach.point(ev.entity, MISS_ATTACH, attacker_tr.translation);
            play(
                &mut kits,
                &mut out,
                kit,
                at,
                PlayExtras {
                    bus: Bus::DEFAULT,
                    ..default()
                },
                "miss whoosh",
            );
            continue;
        }
        // The connecting swing (`0x624c36`).
        let (Some(swings), Some(subs)) = (swing_sounds.as_deref(), sub_classes.as_deref()) else {
            continue;
        };
        let Some(weight) = swing_weight(wielded, offhand, &subs.0) else {
            continue; // a non-weapon in the hand: `0x623870` returns false and nothing plays
        };
        let Some(kit) = swings.0.kit(weight, swing.hit_info & HITINFO_CRITICAL != 0) else {
            continue; // `0x457f63`'s `swingType >= 3` bail: silence, never a fallback weight
        };
        play(
            &mut kits,
            &mut out,
            kit,
            // The event's own point (`0x624c6e` → `0x457f60`), usually on a moving bone.
            ev.pos.unwrap_or(attacker_tr.translation),
            PlayExtras {
                bus: Bus::WEAPON_SWING,
                // Half volume under `HITINFO_MISS` (`0x457f74`/`0x457f7d`); vmangos never
                // sends a miss with a connecting victimState.
                volume_mult: Volume(if swing.hit_info & HITINFO_MISS != 0 {
                    0.5
                } else {
                    1.0
                }),
                ..default()
            },
            "connecting swing",
        );
    }

    // The contact family: the weapon-sound block, then the victim dispatch.
    for imp in contacts.read() {
        if imp.text_only {
            continue; // a supersede/stop flush carries only the floating text
        }
        let swing = &imp.swing;
        let attacker = units.get(swing.attacker).ok();
        let victim = swing.victim.and_then(|v| units.get(v).ok());
        // The fired tag's own point (`0x6248ef`, `0x624950`); the receive-time fallback fired no
        // tag, so it takes the attacker, then the victim.
        let Some(pos) = imp
            .pos
            .or_else(|| attacker.map(|(t, ..)| t.translation))
            .or_else(|| victim.map(|(t, ..)| t.translation))
        else {
            continue;
        };
        let crit = swing.hit_info & HITINFO_CRITICAL != 0;
        if !no_contact(swing) {
            // The attacker's weapon row (`0x625460`); `None` for a wand or thrown weapon.
            let offhand = swing.hit_info & HITINFO_LEFTSWING != 0;
            let (subclass, metal) = swing_weapon(attacker.and_then(|(_, w, ..)| w), offhand, mats);
            let row = impacts.0.get(subclass, metal);
            let defended = defended(swing.victim_state);

            // `0x6247d0`'s weapon-sound block, before the victim dispatch: the `$AHn` column,
            // else the generic impact behind the `SWINGNOHITSOUND` latch. Under a parry or block
            // benilla plays no generic impact; the reference's side (`0x624936`) is untraced.
            if let Some(n) = imp.natural {
                // The natural-weapon sound replaces the generic weapon impact.
                let vocal = attacker
                    .and_then(|(_, _, net, ..)| net.display_id)
                    .and_then(|d| voices.0.for_display(d))
                    .and_then(|v| v.custom_attack.get(usize::from(n)).copied())
                    .unwrap_or(0);
                play(
                    &mut kits,
                    &mut out,
                    vocal,
                    pos,
                    PlayExtras {
                        bus: Bus::MELEE_IMPACT,
                        ..default()
                    },
                    "natural impact",
                );
            } else if !defended {
                if let Some(row) = row {
                    // A player victim presents its chest armor (`0x62fb70`), self-only.
                    let slot = match (victim, mats) {
                        (Some((_, _, vnet, _, vstore)), Some(mats))
                            if vnet.kind == EntityKind::Player =>
                        {
                            items
                                .as_mut()
                                .and_then(|it| {
                                    super::worn_chest_material(vstore, &objects, it, &net_commands)
                                })
                                .map_or(impact_slot::FLESH, |m| mats.armor_impact_slot(m) as usize)
                        }
                        _ => creature_impact_slot(
                            victim
                                .and_then(|(_, _, net, ..)| net.display_id)
                                .and_then(|d| voices.0.for_display(d))
                                .map_or(0, |v| v.impact_type),
                        ),
                    };
                    let kit = landed_impact(row, slot, crit);
                    play(
                        &mut kits,
                        &mut out,
                        kit,
                        pos,
                        PlayExtras {
                            bus: Bus::MELEE_IMPACT,
                            ..default()
                        },
                        "impact",
                    );
                }
            }

            // The clang at the victim (`0x6245bb` parry, `0x6245d8` block), reached on every
            // impact tag through the shared tail (`0x6249bb`), natural weapons included. No
            // defending item skips only the clang: the wound vocal below still runs.
            let defending = defending_item(
                victim.and_then(|(_, w, ..)| w),
                swing.victim_state == VICTIM_BLOCK,
            );
            if let (true, Some(row), Some(item)) = (defended, row, defending) {
                let kit = defense_clang(row, Some(item), mats);
                // The clang is lifted two yards (`0x457e29`: `fadd [0x801628]`).
                let at = victim.map(|(t, ..)| t.translation).unwrap_or(pos) + Vec3::Y * STUB_HEIGHT;
                play(
                    &mut kits,
                    &mut out,
                    kit,
                    at,
                    PlayExtras {
                        bus: Bus::MELEE_IMPACT,
                        ..default()
                    },
                    "defense clang",
                );
            }
        }

        // The stub kits sit outside the contact guard: [`no_contact`] includes deflect and immune.
        let stub_at = |t: &Transform| t.translation + Vec3::Y * STUB_HEIGHT;
        if swing.victim_state == VICTIM_DEFLECT {
            if let Some((victim_tr, ..)) = victim {
                play(
                    &mut kits,
                    &mut out,
                    DEFLECT_KIT,
                    stub_at(victim_tr),
                    PlayExtras::default(),
                    "deflect",
                );
            }
        }
        if swing.hit_info & HITINFO_ABSORB_OR_RESIST != 0 || swing.victim_state == VICTIM_IMMUNE {
            if let Some((victim_tr, ..)) = victim {
                play(
                    &mut kits,
                    &mut out,
                    ABSORB_KIT,
                    stub_at(victim_tr),
                    PlayExtras::default(),
                    "absorb",
                );
            }
        }

        // The wound vocal (`0x62460c`..`0x624674`): absorb, resist and immune never reach it;
        // then `hitInfo & 0x2` (`AFFECTS_VICTIM`) is required, and the class is crushing → 9,
        // critical → 3, `MISS` → nothing, else 2.
        //
        // Deviation: `damage > 0` without a parry or block stands in for `0x2`, because vmangos
        // clears that bit only for immune and a zero-damage miss or absorb
        // (`Unit.cpp:1366`/`1587`), so it would grunt on every parry.
        if swing.damage > 0
            && swing.hit_info & HITINFO_ABSORB_OR_RESIST == 0
            && !matches!(swing.victim_state, VICTIM_PARRY | VICTIM_BLOCK)
        {
            // The class decides both gates below (`0x62462c`/`0x624649`/`0x624666`).
            let crushing = swing.hit_info & HITINFO_CRUSHING != 0;
            // The `AISOUNDDESC` gate (`0x4591f0` from `0x6234cb`): a live server-pushed sound on
            // the victim suppresses classes 0-3 and 8 (`0x6234bb`), so a crushing blow's class 9
            // gets through. The player twin `0x62f880` has no gate.
            let vocal_victim = victim.filter(|(_, _, net, ..)| {
                crushing
                    || net.kind == EntityKind::Player
                    || !swing.victim.is_some_and(|v| object_sound_playing(&out, v))
            });
            if let Some((victim_tr, _, net, victim_is_you, _)) = vocal_victim {
                // The class roll, keyed on the victim's type (`[vt+0x88]`): class 2 is 60 for a
                // creature and 30 for a player; classes 3 and 9 are 100.
                if !crushing && !crit {
                    let threshold = if net.kind == EntityKind::Player {
                        INJURY_CHANCE_PLAYER
                    } else {
                        INJURY_CHANCE_CREATURE
                    };
                    if !bark_chance_pass(threshold, kits.roll()) {
                        continue;
                    }
                }
                // A zero column plays nothing, with no fallback to another class: `0x623490`
                // tests the id once (`0x6234e6`), as does the player twin (`0x62f8be`).
                let vocal = net
                    .display_id
                    .and_then(|d| voices.0.for_display(d))
                    .map(|v| v.injury[if crushing { 2 } else { usize::from(crit) }])
                    .unwrap_or(0);
                // Your own wounds take bus 8 (cap 1), everyone else's bus 7 (cap 2).
                let bus = if victim_is_you {
                    Bus::SELF_INJURY
                } else {
                    Bus::INJURY
                };
                play(
                    &mut kits,
                    &mut out,
                    vocal,
                    victim_tr.translation,
                    PlayExtras { bus, ..default() },
                    "injury",
                );
            }
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Startup,
        (load_weapon_impacts, load_weapon_swings).after(AssetSet::Open),
    )
    .add_systems(Update, combat_sounds.in_set(WorldStage::Present));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x624ca0`'s `{0, 2, 6}`; immune and deflect take the connecting swing.
    #[test]
    fn only_unaffected_dodge_and_evade_take_the_miss_whoosh() {
        for whiff in [0, VICTIM_DODGE, VICTIM_EVADE] {
            assert!(whiffed(whiff), "victimState {whiff} whiffs");
        }
        for connects in [
            1,
            VICTIM_PARRY,
            4,
            VICTIM_BLOCK,
            VICTIM_IMMUNE,
            VICTIM_DEFLECT,
        ] {
            assert!(
                !whiffed(connects),
                "victimState {connects} takes the connecting swing"
            );
        }
    }

    /// `0x623870`'s three answers on the shipped `ItemSubClass.dbc`. Skips without client data.
    #[test]
    fn the_swing_weight_is_the_dbc_column_not_a_guess() {
        let Some(data) = benilla_formats::wow_data() else {
            eprintln!("skipping: no WoW install found");
            return;
        };
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let subs = benilla_formats::load_item_sub_classes(&mut chain).expect("ItemSubClass.dbc");

        let hand = |item: Option<(u8, u8)>| {
            swing_weight(
                Some(&Wielded {
                    main: item,
                    ..default()
                }),
                false,
                &subs,
            )
        };
        // Light: daggers (15) and fist weapons (13).
        assert_eq!(hand(Some((2, 15))), Some(0), "dagger");
        assert_eq!(hand(Some((2, 13))), Some(0), "fist weapon");
        // Medium: the one-handers and the ranged bodies.
        for medium in [0u8, 2, 3, 4, 7, 16, 18, 19] {
            assert_eq!(hand(Some((2, medium))), Some(1), "subclass {medium}");
        }
        // Heavy: every two-hander, plus polearms, staves and spears.
        for heavy in [1u8, 5, 6, 8, 10, 17] {
            assert_eq!(hand(Some((2, heavy))), Some(2), "subclass {heavy}");
        }
        // An empty hand swings Light; a shield (class 4) or any other non-weapon is silent.
        assert_eq!(hand(None), Some(0), "unarmed");
        assert_eq!(hand(Some((4, 6))), None, "shield in hand");
        assert_eq!(hand(Some((0, 0))), None, "consumable in hand");
        // No `Wielded` component at all reads as an empty hand, like the reference's null item.
        assert_eq!(swing_weight(None, false, &subs), Some(0));
    }

    /// The weight picks the `WeaponSwingSounds2` row and the crit bit its column, on shipped
    /// data. Skips without client data.
    #[test]
    fn a_weapons_subclass_reaches_its_own_woosh_kit() {
        let Some(data) = benilla_formats::wow_data() else {
            eprintln!("skipping: no WoW install found");
            return;
        };
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let subs = benilla_formats::load_item_sub_classes(&mut chain).expect("ItemSubClass.dbc");
        let swings =
            benilla_formats::load_weapon_swing_catalog(&mut chain).expect("WeaponSwingSounds2.dbc");

        let kit = |item: Option<(u8, u8)>, crit: bool| {
            let w = swing_weight(
                Some(&Wielded {
                    main: item,
                    ..default()
                }),
                false,
                &subs,
            )?;
            swings.kit(w, crit)
        };
        assert_eq!(kit(Some((2, 15)), false), Some(233), "dagger");
        assert_eq!(kit(Some((2, 15)), true), Some(234), "dagger crit");
        assert_eq!(kit(Some((2, 7)), false), Some(235), "1H sword");
        assert_eq!(kit(Some((2, 7)), true), Some(236), "1H sword crit");
        assert_eq!(kit(Some((2, 8)), false), Some(237), "2H sword");
        assert_eq!(kit(Some((2, 8)), true), Some(238), "2H sword crit");
        assert_eq!(kit(None, false), Some(233), "unarmed swings light");
        assert_eq!(kit(Some((4, 6)), false), None, "a shield makes no whoosh");
    }

    /// The stub and miss kits are the `(DONOTRENAME)` rows the startup cache names. Skips without
    /// client data.
    #[test]
    fn the_deflect_and_absorb_stubs_name_real_kits() {
        let Some(data) = benilla_formats::wow_data() else {
            eprintln!("skipping: no WoW install found");
            return;
        };
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let kits = benilla_formats::load_sound_kit_catalog(&mut chain).expect("SoundEntries.dbc");

        let named = |id: u32| kits.get(id).map(|k| k.name.clone());
        assert_eq!(
            named(DEFLECT_KIT).as_deref(),
            Some("(DONOTRENAME)ShieldWoodImpact")
        );
        assert_eq!(
            named(ABSORB_KIT).as_deref(),
            Some("(DONOTRENAME)AbsorbGetHit")
        );
        assert_eq!(
            named(COMBAT_MISS_1H).as_deref(),
            Some("(DONOTRENAME)Combat Miss 1H")
        );
        assert_eq!(
            named(COMBAT_MISS_2H).as_deref(),
            Some("(DONOTRENAME)Combat Miss 2H")
        );
    }

    /// The shipped Sword1H-metal row's ids.
    fn sword1h_metal() -> benilla_formats::WeaponImpactRow {
        let mut impact = [0u32; 10];
        let mut crit = [0u32; 10];
        impact[impact_slot::FLESH] = 143;
        crit[impact_slot::FLESH] = 144;
        impact[impact_slot::CHAIN] = 145;
        crit[impact_slot::CHAIN] = 146;
        impact[impact_slot::PLATE] = 147;
        crit[impact_slot::PLATE] = 148;
        impact[impact_slot::STONE] = 3206;
        impact[impact_slot::SHIELD_METAL] = 3263;
        crit[impact_slot::SHIELD_METAL] = 3263;
        impact[impact_slot::SHIELD_WOOD] = 3262;
        crit[impact_slot::SHIELD_WOOD] = 3262;
        impact[impact_slot::PARRY_METAL] = 1002;
        impact[impact_slot::PARRY_WOOD] = 1001;
        benilla_formats::WeaponImpactRow { impact, crit }
    }

    /// Skips without client data (the metal test is `Material.dbc`'s).
    #[test]
    fn defense_clang_follows_the_defending_items_class_and_material() {
        let Some(data) = benilla_formats::wow_data() else {
            eprintln!("skipping: no WoW install found");
            return;
        };
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let mats = benilla_formats::load_material_catalog(&mut chain).expect("Material.dbc");
        let m = Some(&mats);
        let row = sword1h_metal();

        // Class 2 = the parry pair; 1 is metal, 2 is wood.
        assert_eq!(defense_clang(&row, Some((2, 1)), m), 1002, "metal weapon");
        assert_eq!(defense_clang(&row, Some((2, 2)), m), 1001, "wooden weapon");
        // Class 4 = the shield pair; a wooden shield is not the metal kit.
        assert_eq!(defense_clang(&row, Some((4, 1)), m), 3263, "metal shield");
        assert_eq!(defense_clang(&row, Some((4, 2)), m), 3262, "wooden shield");
        // Plate and chain shields are metal-flagged; leather and cloth are not.
        assert_eq!(defense_clang(&row, Some((4, 6)), m), 3263, "plate shield");
        assert_eq!(defense_clang(&row, Some((4, 8)), m), 3262, "leather shield");
        // A material the wire never resolved reads non-metal, like the reference's id 0.
        assert_eq!(defense_clang(&row, Some((4, 0)), m), 3262, "no material");
    }

    /// `0x625400(sel)`.
    #[test]
    fn the_defending_item_is_the_hand_the_outcome_names() {
        let sword_and_board = Wielded {
            main: Some((2, 7)),
            off: Some((4, 6)),
            materials: [1, 6, 0],
            ..default()
        };
        assert_eq!(
            defending_item(Some(&sword_and_board), false),
            Some((2, 1)),
            "a parry takes the mainhand sword"
        );
        assert_eq!(
            defending_item(Some(&sword_and_board), true),
            Some((4, 6)),
            "a block takes the offhand shield"
        );

        // A non-weapon mainhand falls through to the offhand on a parry.
        let torch_and_board = Wielded {
            main: Some((0, 0)),
            off: Some((4, 6)),
            materials: [0, 6, 0],
            ..default()
        };
        assert_eq!(defending_item(Some(&torch_and_board), false), Some((4, 6)));

        // Nothing in the offhand, nothing defending: an unarmed parry is silent.
        let bare = Wielded::default();
        assert_eq!(defending_item(Some(&bare), false), None);
        assert_eq!(defending_item(Some(&bare), true), None);
        assert_eq!(defending_item(None, true), None);
    }

    #[test]
    fn only_parry_and_block_are_defended() {
        assert!(defended(VICTIM_PARRY));
        assert!(defended(VICTIM_BLOCK));
        for other in [
            0,
            1,
            VICTIM_DODGE,
            4,
            VICTIM_EVADE,
            VICTIM_IMMUNE,
            VICTIM_DEFLECT,
        ] {
            assert!(!defended(other), "victimState {other} is not a defense");
        }
    }

    #[test]
    fn landed_impact_reads_the_victim_slot_crit_tiered() {
        let row = sword1h_metal();
        assert_eq!(landed_impact(&row, impact_slot::FLESH, false), 143);
        assert_eq!(landed_impact(&row, impact_slot::FLESH, true), 144);
        assert_eq!(landed_impact(&row, impact_slot::CHAIN, false), 145);
        assert_eq!(landed_impact(&row, impact_slot::PLATE, false), 147);
        assert_eq!(landed_impact(&row, impact_slot::PLATE, true), 148);
        assert_eq!(landed_impact(&row, impact_slot::STONE, false), 3206);
    }

    /// `0x6238f0`'s remap never reaches the chain or plate slots.
    #[test]
    fn the_creature_impact_slot_is_the_four_entry_remap() {
        assert_eq!(creature_impact_slot(0), impact_slot::FLESH);
        assert_eq!(creature_impact_slot(1), impact_slot::STONE);
        assert_eq!(creature_impact_slot(2), impact_slot::WOOD);
        assert_eq!(creature_impact_slot(3), impact_slot::ETHEREAL);
        for refused in [4, 5, 99, u32::MAX] {
            assert_eq!(
                creature_impact_slot(refused),
                impact_slot::FLESH,
                "impact type {refused} is past the reference's `>= 4` bail"
            );
        }
        for t in 0..4 {
            let slot = creature_impact_slot(t);
            assert_ne!(slot, impact_slot::CHAIN);
            assert_ne!(slot, impact_slot::PLATE);
        }
    }

    /// The chest armor's slot on shipped data, asserted as the kit heard. Skips without client
    /// data.
    #[test]
    fn a_players_chest_armor_picks_the_chain_and_plate_columns() {
        let Some(data) = benilla_formats::wow_data() else {
            eprintln!("skipping: no WoW install found");
            return;
        };
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let materials = benilla_formats::load_material_catalog(&mut chain).expect("Material.dbc");
        let impacts =
            benilla_formats::load_weapon_impact_catalog(&mut chain).expect("WeaponImpactSounds");
        let sword = impacts.get(7, true).expect("metal 1H sword row");

        let hit = |material: u32| {
            landed_impact(sword, materials.armor_impact_slot(material) as usize, false)
        };
        // Shipped row 8 (subclass 7, metal): flesh 143, chain 145, plate 147.
        assert_eq!(hit(6), 147, "plate chest");
        assert_eq!(hit(5), 145, "chain chest");
        assert_eq!(hit(8), 143, "leather chest rings as flesh");
        assert_eq!(hit(7), 143, "cloth chest rings as flesh");
        assert_eq!(hit(0), 143, "no chest / unstreamed → flesh");
    }
}
