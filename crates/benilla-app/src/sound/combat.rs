//! Combat audio (decisions 0075 + 0525): the melee swing's sounds — the whoosh and exertion on
//! their own event tags, the CONTACT family on the victim dispatch ([`SwingImpact`]).
//!
//! The trigger chain: `SMSG_ATTACKERSTATEUPDATE` → [`SwingMessage`] (net bridge; also drives the
//! swing *animation*, decision 0073) → the attack sequence's M2 events fire mid-swing through
//! [`AnimSoundEvent`]. Two tags route directly from that stream:
//!
//! - **`$CSS`** — the swing whoosh, in **both** of its forms. Nothing contacted (miss/dodge/
//!   evade) gets kits 7080/7081 `Combat Miss 1H/2H` by weapon handedness — exactly the two ids
//!   the client caches by name at startup (`0x4575b0`, `_DONOTRENAME_` kits). Anything
//!   else gets the *connecting* swing's `WeaponSwingSounds2` whoosh by weapon weight; see "The
//!   connecting swing" below.
//! - **`$CAH`** — **not** the attacker's exertion, which is what this module used to think.
//!   `$CAH` drives the **victim's** injury vocal (`0x624865 je 0x624902` → `0x6249bb call
//!   0x624530`), which benilla already fires off that crossing; there is no `call [reg+0x88]`
//!   anywhere in `0x6247d0`, so the tag never reaches the exertion columns at all.
//!
//! The **attacker's exertion** is packet-driven at swing start, and is now wired that way
//! (`SMSG_ATTACKERSTATEUPDATE` → `0x6246a0` → `0x624786` → `0x623b10`). Read off the bytes:
//!
//! - `class = ([hitrec+0x10] >> 7) & 1` — the crit bit, so `Exertion` / `ExertionCritical`.
//! - `0x62476a`: gated on **`victimState != 0`**, and only on that. (The earlier note here also
//!   claimed a victim-health gate; `0x6246a0` contains no such test. Its other two bails —
//!   `[[attacker+0x110]+0x40] <= 0` at `0x6246b1` and `hitInfo & 0x10000` at `0x6246c2` — sit
//!   above the swing *animation* select as well, so they belong to that subsystem, not here.)
//! - `force = 0` (`0x62477e`), so the **class chance roll applies**: threshold 70 for a creature
//!   and 35 for a player on class 0, 100 on class 1. A crit always grunts; an ordinary swing
//!   grunts ~70 % of the time for a creature and ~36 % for a player.
//!
//! This moves *when* a grunt is heard — swing start rather than mid-clip — and thins ordinary
//! swings out, which the tag-driven version never did.
//!
//! The **contact family** consumes [`SwingImpact`] — the victim dispatch `0x624530` + the
//! `0x6247d0` weapon-sound block, fired at the swing clip's **`$AH0–3`/`$CAH`** crossing, or at
//! receive for an unresolved attacker (decision 0529; previously keyed on `$HIT`, a CEffect-only
//! tag many creature attack clips never author — ogre.m2 authors it in 1 of its ~14 attack
//! variations, so ogre hits were near-silent):
//!
//! It is **two blocks in the client's order**, not one pick (decision 0899):
//!
//! 1. `0x6247d0`'s own weapon-sound block, at the attacker — a **natural-weapon** swing
//!    (`$AHn` fired the dispatch) plays the attacker's `CreatureSoundData.CustomAttack[n]`
//!    column INSTEAD of the generic weapon impact (the `SWINGNOHITSOUND` latch, `0x6247d0`);
//!    otherwise a landed hit plays the `WeaponImpactSounds` impact/crit slot for the victim's
//!    material (`CreatureImpactType`).
//! 2. `0x624530`'s **victimState-keyed clang**, at the victim — parry (`0x6245bb` →
//!    `0x623640(sel=0)`) takes the attacker row's parry slot (metal/wood by the victim's
//!    weapon), block (`0x6245d8` → `sel=1`) the shield slot. This block is reached on **every**
//!    impact tag: the `$AHn` digit block "has zero effect on the victim dispatch", so a beast's
//!    bite and the parry clang both sound.
//!
//! Plus the victim's injury vocal (`Injury`/`InjuryCritical`/`InjuryCrushing`) on a damaging,
//! undefended hit — the ordinary one **rolled** at the victim's own class-2 threshold, 60 for a
//! creature and 30 for a player, while a crit and a crushing blow always sound (decision 2073).
//!
//! A `text_only` flush (supersede/attack-stop) drops its sounds — only the floating number
//! flushes (decision 0149's flush law, inherited from the shared dispatch).
//!
//! ## The connecting swing — the other half of `$CSS` (decision 1567)
//!
//! `$CSS` is **two** sounds, not one, and `0x624ca0` picks between them off the victimState
//! alone: `{0 unaffected, 2 dodge, 6 evade}` take the by-handedness miss whoosh, and **every
//! other outcome — a landed hit, a parry, a block, an immune, a deflect — takes
//! `WeaponSwingSounds2` on the capped bus 6** ([`whiffed`]). benilla voiced only the first half
//! until 1567, so every landed melee swing in the game was missing a sound.
//!
//! What blocked it was `0x623870`, the `swingType` source, read now: it is **not a heuristic**.
//! The function asks the swinging hand (`hitInfo & LEFTSWING` picks which) for its item, requires
//! item class 2, and returns `ItemSubClass[(2, subclass)]` field 9 — `WeaponSwingSize`, a shipped
//! DBC column ([`swing_weight`]). The shipped weapon rows put daggers and fist weapons at Light,
//! every two-hander plus polearms, staves and spears at Heavy, and the rest at Medium; an **empty
//! hand returns Light** (`0x623892` writes 0 and returns *true*) and a **non-weapon in hand
//! returns false**, i.e. silence. The kit is then `cache[critical + weight*2]` over the six
//! `WeaponSwingSounds2` rows — 233..238, `mWooshSmall/Medium/Large` — at volume 0.5 when the hit
//! flags carry `HITINFO_MISS` and 1.0 otherwise (`0x457f74`/`0x457f7d`).
//!
//! One benilla behaviour changed beyond the new sound: immune and deflect used to take the *miss*
//! whoosh here, because this branch borrowed [`no_contact`] — the impact family's wider question.
//! `0x624ca0` reads neither the hit flags nor those two states, so they now whoosh like the
//! contacts they are.
//!
//! ## The victim dispatch's tail, read out (1567)
//!
//! `0x624530` past the clang is a four-way ladder, and two of its arms were silent here:
//!
//! - **deflect** (victimState 8) plays a fixed `(DONOTRENAME)ShieldWoodImpact`, id 3262
//!   ([`DEFLECT_KIT`]) — not a weapon-row slot.
//! - **absorb, resist or immune** (`hitInfo & 0x60`, or victimState 7) plays
//!   `(DONOTRENAME)AbsorbGetHit`, id 3334 ([`ABSORB_KIT`]), **instead of** the wound vocal —
//!   it returns before reaching it.
//! - otherwise the wound vocal, keyed crushing → critical → (`MISS` → nothing) → injury.
//!
//! Both stub kits come out of the same startup name cache as the miss whooshes (`0x4575b0`), and
//! both emit at the victim two yards up. One reading is *not* taken literally: see the wound
//! vocal's own note on `HITINFO_AFFECTS_VICTIM` and what vmangos does with it on a parry.
//!
//! ## The material law, and where the ten slots actually come from (1567)
//!
//! Three of this module's INTERIM readings were one table: **`Material.dbc`'s `Flags` column**.
//!
//! - **metal vs wood** is `Flags & 1` (`0x457e80` → `0x5d9a50`), not `material != WOOD`. Leather,
//!   cloth and liquid are non-metal too — and so is the id 0 a creature with no virtual-item
//!   info carries, which the old test called metal.
//! - **the victim's slot** is `[vt+0x90]`, two implementations naming disjoint halves of the ten:
//!   a creature's `CreatureSoundData` column remapped `{0, 8, 7, 9}` (flesh/stone/wood/ethereal),
//!   or a **player's chest armor** (`0x62fb70`: plate → plate, chain → chain, else flesh). The
//!   chain and plate columns are reachable only through a player victim, and only the local one —
//!   the same self-only reach as the foley. benilla sent every player to flesh, which is why a
//!   plate warrior's own hits sounded like a punch on meat.
//! - **the clang's slot family follows the defending item's class**, not the victimState
//!   (`0x457de6`/`0x457dfd`): class 2 the parry pair, class 4 the shield pair, material picking
//!   metal or wood inside it. The item is `0x625400(sel)` — a parry prefers a mainhand weapon and
//!   falls through to the offhand, a block goes straight to the offhand — and **no defending item
//!   means no clang**, so an unarmed parry is silent. benilla read the mainhand for both and
//!   hardcoded the metal shield.
//!
//! One more byte detail: the clang alone is emitted two yards up (`0x457e29`); the generic weapon
//! impact beside it passes its position straight through.
//!
//! INTERIM readings: a defended outcome suppresses block 1's generic weapon impact
//! (the tail's latch test `0x624936` carries no victimState gate in the trace, so whether it
//! also plays under a clang is unpinned);
//! the natural-weapon column is gated on
//! contact like the weapon impact (whether the digit block also plays on a whiff is unpinned).
//! `$CPP`/`$CST` are pinned NON-audio (decision 0279): `$CPP` is the victim defense-anim
//! dispatch, `$CST` re-pings the attached combat-kit list — neither belongs to this module.

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

/// `HitInfo` bit 2 — the **offhand** swing (vmangos `HITINFO_LEFTSWING`), and the reference's own
/// hand selector: `0x624c36` derives `slot = (hitInfo >> 2) & 1` and hands it to `0x623870`, which
/// asks that hand for its item.
const HITINFO_LEFTSWING: u32 = 0x4;

/// The two `_DONOTRENAME_` whoosh kits the client caches by name at startup (`0x4575b0`);
/// byte-verified ids in the 5875 SoundEntries dump.
const COMBAT_MISS_1H: u32 = 7080;
const COMBAT_MISS_2H: u32 = 7081;

/// The two **fixed, name-cached stub kits** of the victim dispatch `0x624530`, resolved from the
/// `(DONOTRENAME)` cache the client fills at startup (`0x4575b0`) — the same cache the miss
/// whooshes come out of. Both play at the **victim**, two yards up, on the uncapped bus 0 at
/// volume 1.0 (`0x458870`), and neither is weapon-row-keyed:
///
/// - **deflect** (`0x6245f5`, victimState 8 → `0x457f20` → `[0xb05fb8]`) →
///   `(DONOTRENAME)ShieldWoodImpact`, id 3262, `WoodenShieldBlock1..3.wav`.
/// - **absorb / resist / immune** (`0x62460f` `hitInfo & 0x60`, or `0x624613` victimState 7 →
///   `0x458610` → `[0xb05fb4]`) → `(DONOTRENAME)AbsorbGetHit`, id 3334,
///   `AbsorbGetHitA/B/C.wav`.
///
/// Both ids are byte-verified in the shipped `SoundEntries` dump against the exact cache strings
/// at `0x835e8c`/`0x835eac`. These were the "unpinned stubs" the module used to leave silent.
const DEFLECT_KIT: u32 = 3262;
const ABSORB_KIT: u32 = 3334;

/// `HitInfo & (HITINFO_ABSORB | HITINFO_RESIST)` — the reference's own `test al, 0x60`, which
/// sends the hit to [`ABSORB_KIT`] instead of the wound vocal.
const HITINFO_ABSORB_OR_RESIST: u32 = 0x60;

/// **The stub emitters' height offset** — `0x457f4a`/`0x45863a` `fadd [0x801628]`, the same flat
/// `2.0` the armor foley uses (`crate::sound::footsteps`). WoW Z is Bevy Y at the same scale.
/// The weapon impact and the parry/block clang do NOT take it: they pass the caller's position
/// straight through, so only the three `0x458870` sites that build a local vector are lifted.
const STUB_HEIGHT: f32 = 2.0;

/// `ItemClass` 2 — a weapon. `0x623870` compares the equipped item's class byte against it
/// (`0x6238b4 cmp byte ptr [eax], 2`) and **returns false** on anything else, so a held
/// non-weapon swings in silence rather than falling back to a weight.
const ITEM_CLASS_WEAPON: u32 = 2;

/// Weapon subclasses swung two-handed (item weapon subclass ids) — picks the 2H whoosh.
const TWO_HANDED: [u32; 6] = [1, 5, 6, 8, 10, 17];
/// Fist/unarmed subclass — the row a weaponless swing uses (`Unarmed_Generic`).
const UNARMED_SUBCLASS: u32 = 13;

/// `WeaponSwingSounds2.dbc` as the reference's own six-slot cache — the connecting swing's kit
/// by `(weight, critical)`. See [`benilla_formats::WeaponSwingCatalog`].
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

/// The latest swing outcome per attacker — written on the packet, read as the `$CSS` event
/// fires over the following frames, overwritten by the next swing. (The contact family does not
/// read it: [`SwingImpact`] carries its own consumed record, decision 0529. Neither does the
/// exertion vocal any more — it fires from the packet itself, so it never needs the record to
/// survive into a later frame.)
#[derive(Default)]
struct LastSwing(EntityHashMap<SwingMessage>);

/// The attacker's swinging weapon: `(subclass, metal)`, unarmed when the hand is empty. The
/// metal-vs-wood half is the item's own **`Material`** off the wire (decision 0882) — not a
/// subclass guess, which the real 5875 data contradicts outright: maces (subclass 4) ship in both
/// materials, so a Cudgel is wood where a Mace is metal.
///
/// **`metal` is `Material.Flags & 1`, not `material != WOOD`** (`0x457e80` → `0x5d9a50`). The
/// distinction is not academic: leather, cloth and liquid are all non-metal, and so is the id 0
/// a creature with no virtual-item info carries — which the old test called metal. An empty hand
/// is non-metal too, matching the reference's null-item path (`0x457e8d` writes the out-param 0).
fn swing_weapon(
    wielded: Option<&Wielded>,
    offhand: bool,
    materials: Option<&benilla_formats::MaterialCatalog>,
) -> (u32, bool) {
    // `0x625400`/`0x625460` pass `visFlag = 0`, so a disarmed hand's weapon is not here to be
    // heard either — the punch a disarmed unit throws must not clang like a sword (1863).
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

/// The victim's `WeaponImpactSounds` **target slot** — the `[vt+0x90]` virtual, whose two
/// implementations name **disjoint halves of the same ten**:
///
/// - a **creature** (`0x6238f0`) reads its `CreatureSoundData` column, refuses anything `>= 4`,
///   and remaps it through the four-entry table at `0x80db94` = `{0, 8, 7, 9}` — flesh, stone,
///   wood, ethereal.
/// - a **player** (`0x62fb70`) ignores that entirely and reads its **chest armor's `Material`**
///   flags: plate → the plate slot, chain → chain, everything else (leather and cloth included)
///   → flesh. Handled at the call site, which is where the chest lookup's data lives.
///
/// So the chain and plate columns of every weapon row are reachable *only* through a player
/// victim, and only the local one — the same self-only reach as the foley
/// ([`super::worn_chest_material`]). benilla used to send every player to flesh, which is why a
/// plate warrior's own hits sounded like a punch on meat.
fn creature_impact_slot(impact_type: u32) -> usize {
    match impact_type {
        1 => impact_slot::STONE,
        2 => impact_slot::WOOD,
        3 => impact_slot::ETHEREAL,
        _ => impact_slot::FLESH,
    }
}

/// **Which whoosh a swing gets** — `0x624ca0`, the reference's own two-way split, and the only
/// input is the victimState. `{0 unaffected, 2 dodge, 6 evade}` take the *miss* whoosh; **every
/// other outcome — a landed hit, a parry, a block, an immune, a deflect — takes the connecting
/// swing's `WeaponSwingSounds2` whoosh** on the capped bus 6. The two are alternatives, never
/// both: the call site branches on this one test (`0x624ba4 je 0x624c36`).
///
/// Deliberately NOT [`no_contact`], which is a wider "nothing for the weapon to strike" question
/// asked of the *impact* family. That predicate also folds in `HITINFO_MISS` and treats immune
/// and deflect as whiffs; `0x624ca0` reads neither the hit flags nor those two states, and
/// sorting a deflect onto the miss whoosh was the audible consequence of borrowing it here.
fn whiffed(victim_state: u32) -> bool {
    matches!(victim_state, 0 | VICTIM_DODGE | VICTIM_EVADE)
}

/// The swinging weapon's **weight** — `0x623870`, verbatim, and the whole of the Light/Medium/
/// Heavy classification benilla could not build before.
///
/// It is not a heuristic and never was: the function asks the swinging hand for its item, checks
/// the class byte is 2, and returns `ItemSubClass[(2, subclass)].WeaponSwingSize` — a shipped DBC
/// column (`[row+0x24]`, field 9) that puts daggers and fist weapons at Light, every two-hander
/// plus polearms, staves and spears at Heavy, and the rest at Medium.
///
/// Three outcomes, each the reference's own:
/// - **empty hand** → `Some(0)`, Light. `0x623892` writes 0 into the out-param and returns
///   *true*, so an unarmed swing whooshes with the small `mWooshSmall*` samples.
/// - **a weapon** → its row's weight, or `None` when the pair has no row.
/// - **a non-weapon in hand** → `None`. `0x6238b7` returns false and the caller plays nothing.
fn swing_weight(
    wielded: Option<&Wielded>,
    offhand: bool,
    sub_classes: &benilla_formats::ItemSubClassCatalog,
) -> Option<u32> {
    // `0x623870`, `visFlag = 0` again: the disarmed hand whooshes with the small unarmed
    // samples, by this function's own empty-hand leg (1863).
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

/// The whiff/no-weapon-contact family: nothing for the weapon to strike. Immune and deflect
/// route to positioned stubs in the client (`0x457f20`/`0x458610`, decision 0279) whose kit ids
/// are unpinned — grouped here (whoosh, no impact) as the INTERIM stand-in.
fn no_contact(swing: &SwingMessage) -> bool {
    swing.hit_info & HITINFO_MISS != 0
        || matches!(
            swing.victim_state,
            VICTIM_DODGE | VICTIM_EVADE | VICTIM_IMMUNE | VICTIM_DEFLECT
        )
}

/// A defended outcome — parry or block. These take their sound from the victim dispatch's
/// victimState-keyed clang ([`defense_clang`]), never from the attacker's weapon-sound block.
fn defended(victim_state: u32) -> bool {
    matches!(victim_state, VICTIM_PARRY | VICTIM_BLOCK)
}

/// The victim's **defending item** — `0x625400(sel)`, `(class, material)`.
///
/// A **parry** prefers the mainhand when it holds a weapon and otherwise falls through to the
/// offhand; a **block** goes straight to the offhand. Either way the item must be a weapon
/// (class 2) or armor (class 4) — and `None` here means the reference plays **no clang at all**
/// (`0x623690 test eax,eax; je`), which is what an unarmed parry sounds like.
fn defending_item(wielded: Option<&Wielded>, block: bool) -> Option<(u8, u8)> {
    let w = wielded?;
    // Both probes are `0x625400(sel)` with `visFlag = 0` (1863): a disarmed parry finds no
    // mainhand weapon and falls through, and an unarmed parry rings nothing at all.
    if !block {
        if let Some((2, _)) = w.armed_main() {
            return Some((2, w.materials[0]));
        }
    }
    let (class, _) = w.armed_off()?;
    matches!(class, 2 | 4).then_some((class, w.materials[1]))
}

/// `0x624530`'s clang: the attacker's weapon row × the victim's defending item (`0x623640` →
/// `0x457dc0`). Crit does not tier it — the parry/shield columns carry the same kit in both
/// tables.
///
/// **The slot family follows the item's own class, not the victimState** (`0x457de6`/`0x457dfd`):
/// class 2 takes the parry pair, class 4 the shield pair, and the item's `Material` picks
/// metal or wood within it. So a dual-wielder who *blocks* with a weapon rings the parry slot,
/// and a wooden shield rings 3262 rather than 3263 — benilla used to read the victim's mainhand
/// for both outcomes and hardcode the metal shield, which got the whole block case wrong.
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

/// `0x6247d0`'s generic weapon impact for a landed hit: the victim's slot
/// ([`creature_impact_slot`] or the player's armor slot) off the attacker's weapon row,
/// crit-tiered.
fn landed_impact(row: &benilla_formats::WeaponImpactRow, slot: usize, crit: bool) -> u32 {
    if crit {
        row.crit[slot]
    } else {
        row.impact[slot]
    }
}

/// What a combat sound needs to know about a unit: where it is, what it wields, what it *is*,
/// whether it is you — and its descriptor store, which rides along only for the victim's chest
/// armor (`0x62fb70`; see [`creature_impact_slot`] on the two halves of `[vt+0x90]`).
type CombatUnit = (
    &'static Transform,
    Option<&'static Wielded>,
    &'static NetEntity,
    Has<Embodied>,
    Option<&'static crate::net::ObjectStore>,
);

/// The five DBC-backed tables the melee sounds read, bundled so the system stays under Bevy's
/// parameter ceiling — and because they are one thing: the melee sound vocabulary. Each is
/// independently optional, like every DBC-backed resource here; absent, its own branch goes
/// quiet rather than the system failing.
/// The attachment a **whiffed** swing's whoosh is born at — `0x624bdd call 0x712cb0(1)`.
const MISS_ATTACH: u16 = 1;

#[derive(bevy::ecs::system::SystemParam)]
struct MeleeTables<'w> {
    impacts: Option<Res<'w, WeaponImpacts>>,
    swing_sounds: Option<Res<'w, WeaponSwings>>,
    materials: Option<Res<'w, super::Materials>>,
    /// One load, two consumers: the tooltip's slot|type line owns this resource
    /// ([`crate::ui_items::ItemSubClasses`]) and the swing whoosh reads the same rows' weight
    /// column. A second loader over one DBC is how a schema quietly drifts.
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
    // The attacker's exertion vocal is **packet-driven**, not tag-driven (see the module note):
    // `SMSG_ATTACKERSTATEUPDATE` -> `0x6246a0` -> `0x624786`. Collected here with the swing record
    // so the vocal fires at swing start, which is where the reference puts it.
    let mut exertions: Vec<(Entity, bool)> = Vec::new();
    for s in swings.read() {
        last.0.insert(s.attacker, *s);
        // `0x62476a`: the vocal leg is gated on victimState, and ONLY on victimState — read off
        // the bytes rather than the second-hand note, which also claimed a victim-health gate
        // that is not in the function. A swing that contacted nothing at all is silent; every
        // other outcome, hit or parried or blocked, grunts.
        if s.victim_state != 0 {
            exertions.push((s.attacker, s.hit_info & HITINFO_CRITICAL != 0));
        }
    }
    // Bound the map: entries for despawned attackers die with the entity check below; a cheap
    // periodic sweep keeps a long session from accumulating dead keys.
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
    // `Material.dbc` decides the metal/wood half of every weapon row and the armor slot a player
    // victim presents. Absent (a DBC that failed to load), every weapon reads non-metal and every
    // victim reads flesh — the reference's own answers for an unknown material, so the fallback
    // is its behaviour rather than a guess of ours.
    let mats = materials.as_deref().map(|m| &m.0);
    // Every combat play carries its **voice bus** (decision 1555): the melee-contact family all
    // contends for bus 10's four voices, the vocals for their own one or two, and the miss whoosh
    // for nothing at all. A kit refused at the cap is not an error — it is the gate doing its job,
    // and `play_kit_ext` reports it as an ordinary silent success.
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

    // The attacker's exertion vocal, at swing start. `force = 0` at `0x62477e`, so the class
    // chance roll applies: class 0 is 70 for a creature and 35 for a player, class 1
    // (ExertionCritical) is 100 in both twins — a crit always grunts, an ordinary swing thins out,
    // and a player grunts about half as often as a creature.
    for (attacker, crit) in exertions {
        let Ok((tr, _, net, _, _)) = units.get(attacker) else {
            continue;
        };
        // Same `AISOUNDDESC` gate, on the attacker this time — exertion is classes 0/1.
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

    // The one tag this module still consumes: the swing whoosh — **both** of them. `0x624ca0`
    // splits the tag two ways by victimState ([`whiffed`]) and the branches are exclusive: a
    // swing that touched nothing gets the by-handedness miss whoosh on the uncapped bus 0, and
    // every other swing gets `WeaponSwingSounds2` by weapon weight on bus 6's cap of 2.
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
            // **ATTACHMENT 1, not the fired key** — the one place in the whole event-position
            // table where a *whiff* and a *hit* of the same tag disagree (`0x624baa`:
            // `0x624bdd call 0x712cb0(1)`, else `GetPosition`). The whoosh comes from the hand,
            // the impact from where the weapon met something.
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
        // The connecting swing (`0x624c36`). Both catalogs are optional like every DBC-backed
        // resource: without them this swing is silent, which is the pre-1567 behaviour.
        let (Some(swings), Some(subs)) = (swing_sounds.as_deref(), sub_classes.as_deref()) else {
            continue;
        };
        let Some(weight) = swing_weight(wielded, offhand, &subs.0) else {
            continue; // a non-weapon in the hand: `0x623870` returns false and nothing plays
        };
        let Some(kit) = swings.0.kit(weight, swing.hit_info & HITINFO_CRITICAL != 0) else {
            continue; // `0x457f63`'s `swingType >= 3` bail — silence, never a fallback weight
        };
        play(
            &mut kits,
            &mut out,
            kit,
            // **EVENT POINT** — `0x624c6e mov ecx,[ebx+0x10]` straight into `0x457f60`, which
            // pushes it through to `0x458890`. 353 of the 363 shipped `$CSS` records sit off
            // their model's origin, on a moving bone; Thunderaan's is 26.4 yd out.
            ev.pos.unwrap_or(attacker_tr.translation),
            PlayExtras {
                bus: Bus::WEAPON_SWING,
                // `0x457f74`/`0x457f7d`: half volume when the hit flags carry `HITINFO_MISS`,
                // full otherwise. Structurally that arm needs a MISS *with* a connecting
                // victimState, which vmangos does not produce — carried anyway because it is
                // what the bytes say, and it costs one field.
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

    // The contact family: the weapon-sound block + victim dispatch, at the impact crossing.
    for imp in contacts.read() {
        if imp.text_only {
            continue; // a supersede/stop flush carries only the floating text
        }
        let swing = &imp.swing;
        let attacker = units.get(swing.attacker).ok();
        let victim = swing.victim.and_then(|v| units.get(v).ok());
        // **The fired tag's own point** (`edi = [ebx+0x10]`, pushed at `0x6248ef` for the
        // CustomAttack column and `0x624950` for the generic weapon impact). A big creature's
        // `$AH1` sits 4.7 yd out on the jaw
        // (`trex.m2`) and Thunderaan's `$CAH` 26.4 yd out; the attacker's origin was standing in
        // for both. The receive-time fallback carries no point — no tag fired — so it keeps the
        // attacker, then the victim, which is the only anchor that packet leaves us.
        let Some(pos) = imp
            .pos
            .or_else(|| attacker.map(|(t, ..)| t.translation))
            .or_else(|| victim.map(|(t, ..)| t.translation))
        else {
            continue;
        };
        let crit = swing.hit_info & HITINFO_CRITICAL != 0;
        if !no_contact(swing) {
            // The attacker's weapon row (`0x625460(attacker, leftswing)`) — shared by both
            // blocks below. `None` for a wand/thrown: no melee row, nothing to strike with.
            let offhand = swing.hit_info & HITINFO_LEFTSWING != 0;
            let (subclass, metal) = swing_weapon(attacker.and_then(|(_, w, ..)| w), offhand, mats);
            let row = impacts.0.get(subclass, metal);
            let defended = defended(swing.victim_state);

            // `0x6247d0`'s own weapon-sound block, BEFORE the victim dispatch: the `$AHn` digit
            // column, else the generic `WeaponImpactSounds` impact behind the SWINGNOHITSOUND
            // latch. A defended outcome takes its sound from the dispatch below instead
            // (INTERIM, decision 0899: the tail's latch test `0x624936` carries no victimState
            // gate in the trace, so whether the generic impact ALSO plays under a clang is
            // unpinned — we suppress, which is what the game sounds like).
            if let Some(n) = imp.natural {
                // The attacker's own natural-weapon sound replaces the generic weapon impact.
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
                    // A landed hit: the victim's own slot. A PLAYER victim presents its chest
                    // armor (`0x62fb70`) — the only route to the chain and plate columns, and
                    // self-only because the inv-slot array is; everything else takes the
                    // creature column's remap.
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

            // `0x624530`'s victimState-keyed clang (`0x6245bb` parry → `0x623640(sel=0)`,
            // `0x6245d8` block → `sel=1`), emitted at the VICTIM (`vtable+0x14` on `this`).
            // Reached on EVERY impact tag — the `$AHn` digit block "has zero effect on the
            // victim dispatch" (every digit reaches it through the shared tail, `0x6249bb`): a
            // wolf's bite and the parry clang both sound. Decision 0899 — 0525 wrongly let the
            // natural column swallow this, so every beast you parried was silent.
            // `0x625400(sel)` on the victim, then `0x457dc0`'s class+material slot pick. No
            // defending item means no clang — the reference bails before the play — and that is
            // a `None` here rather than a `continue`, because the wound vocal below still has to
            // run. (An unarmed parry is silent; it is not a silent hit.)
            let defending = defending_item(
                victim.and_then(|(_, w, ..)| w),
                swing.victim_state == VICTIM_BLOCK,
            );
            if let (true, Some(row), Some(item)) = (defended, row, defending) {
                let kit = defense_clang(row, Some(item), mats);
                // The clang alone is lifted two yards (`0x457e29 fadd [0x801628]`); the generic
                // weapon impact next to it passes its position straight through.
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

        // The dispatch's two **fixed stub kits** ([`DEFLECT_KIT`] / [`ABSORB_KIT`]) — outside the
        // contact guard above on purpose: `0x624530` reaches them on every impact, and
        // [`no_contact`] sorts deflect and immune into the whiff family, so a guarded placement
        // would leave exactly these two branches silent, which is what they were.
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

        // The victim's wound vocal, the tail of the same dispatch. The reference's ladder is
        // fully read now (`0x62460c`..`0x624674`): absorb/resist/immune leave through the stub
        // above and never reach it; then `hitInfo & AFFECTS_VICTIM (0x2)` is required, and the
        // class is crushing `0x8000` → 9, else critical `0x80` → 3, else `MISS 0x10` → nothing,
        // else 2.
        //
        // **`damage > 0` stands in for that `0x2`, deliberately** (and the parry/block
        // suppression with it). vmangos leaves `HITINFO_AFFECTS_VICTIM` *set* on a parry and a
        // dodge — it only clears it for immune and for zero damage with MISS|ABSORB
        // (`Unit.cpp:1366`/`1587`), where its own comment calls the bit "no being hit animation
        // on victim without it". Taking `0x2` literally against this server would therefore
        // grunt on every parry, which is audibly wrong and is not what the bit meant. The
        // reference's ladder is faithful to the reference; this gate is faithful to the sound.
        if swing.damage > 0
            && swing.hit_info & HITINFO_ABSORB_OR_RESIST == 0
            && !matches!(swing.victim_state, VICTIM_PARRY | VICTIM_BLOCK)
        {
            // The vocal's **class**, which decides both gates below: crushing `0x8000` → 9,
            // else critical `0x80` → 3, else 2 (`0x62462c`/`0x624649`/`0x624666`).
            let crushing = swing.hit_info & HITINFO_CRUSHING != 0;
            // The `AISOUNDDESC` gate (`0x4591f0` from `0x6234cb`): a server-pushed object sound
            // live on the victim suppresses its own vocal — **classes 0-3 and 8 only**
            // (`0x6234bb`/`0x6234bf`/`0x6234c4`), so a CRUSHING blow's class 9 punches through a
            // scripted voice line where an ordinary hit and a crit do not. The CGPlayer twin
            // `0x62f880` omits the gate, so a player is never suppressed. Filtered off the victim
            // rather than `continue`d, because everything else in this iteration still stands.
            let vocal_victim = victim.filter(|(_, _, net, ..)| {
                crushing
                    || net.kind == EntityKind::Player
                    || !swing.victim.is_some_and(|v| object_sound_playing(&out, v))
            });
            if let Some((victim_tr, _, net, victim_is_you, _)) = vocal_victim {
                // The class chance roll (decision 2073), inside the victim's own `[vt+0x88]` and
                // therefore keyed on the VICTIM's type: class 2 is 60 for a creature and 30 for a
                // player, while classes 3 and 9 carry 100 and always sound. So an ordinary wound
                // grunt thins out under sustained melee and the big hits punch through it.
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
                // **A zero column plays NOTHING** (decision 2075). benilla used to walk down the
                // family here — crushing → critical → ordinary — on the reasoning that "crushing
                // rows are often 0 in data"; they are, and the reference is simply silent for
                // them. `0x623490` tests the selected id exactly once (`0x6234e6 test ebx,ebx ;
                // 0x6234e8 je 0x62350e`, the shared epilogue), and no fallback is even reachable:
                // the row pointer dies at `0x6234e4`, so neither the bus pick `0x623b10` (which
                // takes the category) nor the play `0x458890` (bus, kit, &pos) ever sees the row
                // to re-read a column from. The CGPlayer twin `0x62f880` is identical
                // (`0x62f8be`/`0x62f8c3`).
                let vocal = net
                    .display_id
                    .and_then(|d| voices.0.for_display(d))
                    .map(|v| v.injury[if crushing { 2 } else { usize::from(crit) }])
                    .unwrap_or(0);
                // Your own wounds get the CGPlayer twin's private bus 8 (cap 1); everyone
                // else's share the world's bus 7 (cap 2).
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

    /// **The `$CSS` split** — `0x624ca0`'s `{0, 2, 6}`, and only that. The two outcomes this
    /// pins hardest are immune and deflect: benilla used to sort them onto the miss whoosh
    /// (via [`no_contact`], which is the *impact* family's question), where the reference sends
    /// them through the connecting swing like any other contact.
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

    /// `0x623870`'s three answers, on the real shipped `ItemSubClass.dbc`: an empty hand is
    /// Light (the out-param is written 0 and the function returns *true*), a weapon is its
    /// subclass's `WeaponSwingSize`, and a **non-weapon in hand is `None`** — the function
    /// returns false there, so a held misc item swings in silence rather than borrowing a
    /// weight. Skips without client data.
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

    /// The join the whole leg rests on, end to end on shipped data: the weight column picks a
    /// row of `WeaponSwingSounds2`, and the crit bit picks its column. A dagger crit is
    /// `LightWeaponCritical`; a two-handed sword is `HeavyWeaponNormal`; unarmed is Light.
    /// Skips without client data.
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

    /// The two fixed stub kits resolve to the exact `(DONOTRENAME)` rows the client's startup
    /// cache names — the join that used to be the "unpinned kit id" leaving both branches
    /// silent. Skips without client data.
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
        // …and they are the ids the miss whooshes sit beside in the same cache, so a rename in
        // shipped data would take all four out together rather than one silently.
        assert_eq!(
            named(COMBAT_MISS_1H).as_deref(),
            Some("(DONOTRENAME)Combat Miss 1H")
        );
        assert_eq!(
            named(COMBAT_MISS_2H).as_deref(),
            Some("(DONOTRENAME)Combat Miss 2H")
        );
    }

    /// A row shaped like the real 5875 Sword1H-metal row (byte-verified ids): flesh 143/144,
    /// chain 145/146, plate 147/148, parry-metal 1002, parry-wood 1001, shields 3263/3262.
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

    /// The clang's family follows the **defending item's class**, and metal-vs-wood follows its
    /// `Material` — including on a shield, which benilla used to hardcode to metal. Skips
    /// without client data (the metal test is `Material.dbc`'s).
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
        // Class 4 = the shield pair — and a wooden shield is NOT the metal kit.
        assert_eq!(defense_clang(&row, Some((4, 1)), m), 3263, "metal shield");
        assert_eq!(defense_clang(&row, Some((4, 2)), m), 3262, "wooden shield");
        // Plate and chain shields are metal-flagged; leather and cloth are not.
        assert_eq!(defense_clang(&row, Some((4, 6)), m), 3263, "plate shield");
        assert_eq!(defense_clang(&row, Some((4, 8)), m), 3262, "leather shield");
        // A material the wire never resolved reads non-metal, like the reference's id 0.
        assert_eq!(defense_clang(&row, Some((4, 0)), m), 3262, "no material");
    }

    /// `0x625400(sel)`: a parry prefers a mainhand **weapon** and otherwise falls through to the
    /// offhand; a block goes straight to the offhand. Nothing defending = no clang.
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

        // Nothing in the offhand, nothing defending — an unarmed parry is silent.
        let bare = Wielded::default();
        assert_eq!(defending_item(Some(&bare), false), None);
        assert_eq!(defending_item(Some(&bare), true), None);
        assert_eq!(defending_item(None, true), None);
    }

    /// Parry and block are the defended pair — the outcomes whose sound comes from the victim
    /// dispatch's clang instead of the attacker's weapon-sound block. A landed hit is not.
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

    /// The landed hit indexes the victim's slot directly, crit-tiered.
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

    /// The **creature** half of `[vt+0x90]` (`0x6238f0`): the `CreatureSoundData` column through
    /// the four-entry remap `{0, 8, 7, 9}`, with everything `>= 4` refused back to flesh. It
    /// reaches flesh, stone, wood and ethereal — and can never reach the chain or plate columns,
    /// which is what makes the player override the only route to them.
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

    /// The **player** half, on the shipped `Material.dbc`: the chest armor's slot. This is the
    /// leg that used to send every player victim to flesh — a plate warrior's own hits sounded
    /// like a punch on meat. Joined here to the real weapon row so the assertion is the kit you
    /// actually hear, not an index. Skips without client data.
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
        // Real 5875 row 8 (subclass 7, metal): flesh 143, chain 145, plate 147.
        assert_eq!(hit(6), 147, "plate chest");
        assert_eq!(hit(5), 145, "chain chest");
        assert_eq!(hit(8), 143, "leather chest rings as flesh");
        assert_eq!(hit(7), 143, "cloth chest rings as flesh");
        assert_eq!(hit(0), 143, "no chest / unstreamed → flesh");
    }
}
