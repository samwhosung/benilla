//! The hunter pet's **paper-doll stat block** — `GetPetHappiness`, `GetPetLoyalty`,
//! `GetPetTrainingPoints`, `GetPetExperience` and `HasPetUI`.
//!
//! Its own module rather than more of [`crate::ui_pet`] because it is a different concern on a
//! different clock: the action bar's contents arrive whole in `SMSG_PET_SPELLS` and change when
//! the server says so, while these five read plain descriptor fields plus two DBC tables and move
//! continuously — happiness drifts every few seconds while a pet is out. Tying them together would
//! make the bar's diff-and-fire churn on numbers no button draws.
//!
//! **One gate governs all four stat bindings**: `0x6116e0(pet)`, which resolves the pet, requires
//! its `UNIT_FIELD_PETNUMBER`, requires its owner to be us, and finishes
//! `cmp byte [player.fields + 0x79], 3; sete al` — `UNIT_FIELD_BYTES_0` byte 1, the class, against
//! **Hunter**. (The enum is pinned by the combo-point gate's own class test on the same byte:
//! `== 4 || == 0xB` is Rogue and Druid, `GetComboPoints 0x51a190`.) A warlock's imp therefore
//! resolves perfectly and still answers nothing — happiness, loyalty and training points are
//! hunter machinery — and each binding says "nothing" in its own way ([`benilla_ui::script::PetStats`]).
//!
//! **Two more bindings ride here off the pet's `CreatureFamily.dbc` row**, and they
//! sit on **opposite sides of that gate** — which is the whole reason they are worth naming
//! together. `UnitCreatureFamily("pet")` (`0x51a310`) has no class test whatsoever, so a warlock's
//! imp shows "Imp" on the page's level line; `GetPetFoodTypes()` (`0x4bea10`) shares the very same
//! `0x6116e0` gate as the four stats above, so a minion — or a charmed beast under a non-hunter —
//! answers an empty diet even when its family row has a food mask. Both are one lookup on the same
//! clock as the block above, pushed by the same setter, which is why they live here rather than in
//! [`crate::ui_pet`]'s token feed: splitting them would mean two systems racing the same
//! [`crate::names::NameCache`] entry for the same answer. See [`family_for`] for the one thing that
//! is not obvious: a pet's creature-template entry is its *descriptor's*, never its guid's.

use bevy::prelude::*;

use benilla_ui::script::{PetStats, ScriptValue, UiScript};

use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore};
use crate::ui_pet::{PetBar, PetUnit};
use crate::ui_unit::UnitFeed;

/// `UNIT_FIELD_BYTES_0` byte 1 == 3 — **Hunter**, the class the four stat bindings gate on
/// (`0x611752`). Pinned rather than assumed: the same byte's `4`/`0xB` are Rogue and Druid in the
/// combo-point gate (`GetComboPoints 0x51a190`), which fixes the enum this value sits in.
const CLASS_HUNTER: u8 = 3;

/// `UNIT_FIELD_BYTES_0` byte 3 == 4 — the **happiness** power, the field `GetPetHappiness`
/// thresholds. Read by power *index*, not by the unit's active power type: a pet's displayed bar is
/// focus, and happiness is a parallel slot that is always there.
const POWER_HAPPINESS: u8 = 4;

/// The two pet DBC tables, loaded once at startup. Absent client data leaves the stat block empty,
/// which is the same shape as "not a hunter pet" — degraded, never wrong.
#[derive(Resource)]
pub(crate) struct PetStatTables {
    pub(crate) personalities: benilla_formats::PetPersonalities,
    pub(crate) loyalty: benilla_formats::PetLoyaltyNames,
}

/// The **family** pair — `CreatureFamily.dbc` (the word) and `ItemPetFood.dbc` (the diet the row's
/// mask names).
///
/// A separate resource from [`PetStatTables`] rather than four fields on it, so the two halves
/// degrade independently: a missing `ItemPetFood.dbc` must not take happiness down with it, and a
/// missing `PetPersonality.dbc` must not blank the level line.
#[derive(Resource)]
pub(crate) struct PetFamilyTables {
    pub(crate) families: benilla_formats::CreatureFamilies,
    pub(crate) foods: benilla_formats::PetFoodNames,
}

/// **The pet snapshot must be pushed before anything fires an event whose handlers read it**.
/// `fire_event` dispatches the Lua handlers *synchronously*, so a system that
/// fires while this frame's `set_pet_stats` is still pending hands the VM last frame's answer —
/// the codebase's own "push before firing" rule (`crate::ui_unit`), which held *inside* every
/// feed but not *between* them.
///
/// It cost the Pet tab. `PetTab_Update` is edge-driven off `UNIT_PET` / `PET_BAR_UPDATE`
/// and its whole predicate is `HasPetUI()`, which is this snapshot. With the three pet feeds
/// merely unordered inside [`UnitFeed`], a cold first summon fired both edges while `HasPetUI()`
/// still answered "no pet"; the push landed after, and since neither edge repeats, the tab stayed
/// down for the rest of the session. Measured live, frame by frame: both events on frame 440 with
/// `HasPetUI()=(nil,nil)`, the flip to `(1,nil)` on 441, and the tab still hidden 48 s later —
/// while calling `PetTab_Update()` by hand raised it instantly.
///
/// A set rather than a pair of `.before()` calls because the constraint belongs to the *snapshot*,
/// not to today's two consumers: anything that later fires a pet event inherits it by ordering
/// after this, and does not have to rediscover why.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PetSnapshot;

pub(crate) struct UiPetStatsPlugin;

impl Plugin for UiPetStatsPlugin {
    fn build(&self, app: &mut App) {
        // Rides the unit feed beside the pet bar's own, and before the VM ticks — the pet frame
        // repaints out of the same pass that pushes its health.
        app.add_systems(Update, feed_pet_stats.in_set(UnitFeed).in_set(PetSnapshot));
    }
}

/// The pet's family word and diet, resolved from its cached creature template.
///
/// **A pet's creature template entry is its descriptor's `OBJECT_FIELD_ENTRY`, never its guid.**
/// A `HIGHGUID_PET` guid carries a *pet number* in the entry-shaped slot (`crate::guid::pet_number`
/// — vmangos `Pet::Create` feeds `Object::_Create`'s entry parameter `petNumber`), which is exactly
/// why [`NameCache::resolve`] sends a pet-*name* query for one; but `Creature::InitEntry` still
/// writes the real template id into the descriptor (`Creature.cpp:376`, `SetEntry(entry) // normal
/// entry always`), so the template query has a key after all. That is the whole reason this reaches
/// past the guid: an Imp's descriptor says entry 416, whose template says `pet_family` 23, whose
/// `CreatureFamily.dbc` row says "Imp".
///
/// This is the **resolving** read — it issues the ask-once creature query on a miss — and it is the
/// only one for the pet, so the answer lands in the shared cache for anything else that wants it.
/// A miss returns the all-absent triple, which is the same shape as "this template has no family":
/// the binding's nil either way, and the query is in flight for the next frame.
///
/// The **icon** rides here rather than in a lookup of its own because it comes off the very same
/// family row as the word — `CreatureFamily.dbc`'s own icon column, which is a pet's only icon
/// (there is no item behind it). `GetPetIcon`'s answer, and the stable window's slot art.
fn family_for(
    pet: Option<&ObjectStore>,
    names: &NameCache,
    commands: &NetCommands,
    tables: Option<&PetFamilyTables>,
) -> (Option<String>, Option<String>, Vec<String>) {
    let Some(entry) = pet.and_then(|s| s.0.object_entry()).filter(|&e| e != 0) else {
        return (None, None, Vec::new());
    };
    // The guid argument is the query body's second field and the server ignores it entirely
    // (vmangos `HandleCreatureQueryOpcode` answers off `packet.entry` alone), so the template-only
    // `0` convention `Items::template` already uses applies here too.
    let _ = names.resolve_creature(entry, 0, commands);
    let Some(tables) = tables else {
        return (None, None, Vec::new());
    };
    let Some(family_id) = names.creature_record(entry).map(|r| r.pet_family) else {
        return (None, None, Vec::new());
    };
    let Some(family) = tables.families.get(family_id) else {
        return (None, None, Vec::new());
    };
    (
        Some(family.name.clone()),
        tables.families.icon(family_id).map(str::to_string),
        tables
            .foods
            .for_mask(family.pet_food_mask)
            .into_iter()
            .map(str::to_string)
            .collect(),
    )
}

/// Resolve the whole stat block for the current pet, or [`PetStats::default`] when there is none.
///
/// Split from the system so the composition is testable: every one of the five *stat* values passes
/// through the *same* `hunter_pet` gate, and a bug that let one leak past it would show up as a
/// warlock with a loyalty level rather than as a compile error.
///
/// `family` ([`family_for`]'s pair) is threaded in already resolved, and the two halves land on
/// **opposite sides of the hunter gate** — the word survives it (`UnitCreatureFamily` has no class
/// test), the diet does not (`GetPetFoodTypes` shares `0x6116e0`). See the gate's own comment.
fn stats_for(
    pet: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
    tables: Option<&PetStatTables>,
    family: (Option<String>, Option<String>, Vec<String>),
) -> (bool, PetStats) {
    let (family, icon, food_types) = family;
    let Some(fields) = pet.map(|s| &s.0) else {
        return (false, PetStats::default());
    };
    // `HasPetUI`'s FIRST return (`0x4be697`): a pet that resolves AND carries a pet number. Not the
    // action bar's gate, which is the cached guid alone — a possessed creature has a bar and no
    // pet number, so it gets buttons and no paper doll.
    let has_ui = fields.unit_is_pet_or_charm();
    // …and the second: the owner's class. We only ever hold a bar for a pet the server named us
    // the controller of, so the owner leg is structurally true here; the class is the live test.
    let hunter = has_ui
        && self_store.map(|s| ((s.0.unit_bytes_0().unwrap_or(0) >> 8) & 0xff) as u8)
            == Some(CLASS_HUNTER);
    if !hunter {
        // **The family WORD rides past the gate; the DIET does not** — and the split is the
        // reference's, not chosen. `UnitCreatureFamily 0x51a310` has no class test at all (its
        // only nil paths are "no record / id 0 / out of range / a null row"), so a warlock's
        // minion shows "Imp" on the page's level line. `GetPetFoodTypes 0x4bea10` is gated on
        // `0x6116e0(pet)` — the same owner-is-me + class-is-Hunter gate as the four stat
        // bindings — so it answers *nothing* for a minion even though the word is there. The
        // shipped data hides the difference for warlocks (every minion family ships food mask
        // 0), but not for a **charmed beast under a non-hunter**: a mind-controlled boar has
        // family 5 and mask 63, and the reference still answers an empty diet for it.
        return (
            has_ui,
            PetStats {
                family,
                // The icon rides past the gate with the WORD, not with the diet — same family
                // row, same reasoning (the placement is INFERRED, see `PetStats`).
                icon,
                ..PetStats::default()
            },
        );
    }
    let (happiness, damage_percentage, loyalty_rate) = tables
        .and_then(|t| t.personalities.for_pet(None))
        .map(|p| {
            let h = p.happiness(fields.unit_power(POWER_HAPPINESS).unwrap_or(0));
            (Some(h.bucket), h.damage_percentage, h.loyalty_rate)
        })
        // No DBC ⇒ the binding's own gate-failure numbers, and nil for the bucket.
        .unwrap_or((None, 100.0, 0.0));
    (
        has_ui,
        PetStats {
            hunter_pet: true,
            happiness,
            damage_percentage,
            loyalty_rate,
            loyalty: tables.and_then(|t| {
                t.loyalty
                    .name(u32::from(fields.unit_loyalty_level()))
                    .map(str::to_string)
            }),
            training_points: fields.unit_training_points(),
            experience: fields.unit_pet_experience(),
            family,
            icon,
            food_types,
        },
    )
}

fn feed_pet_stats(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    pet: PetUnit,
    self_store: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    tables: Option<Res<PetStatTables>>,
    family_tables: Option<Res<PetFamilyTables>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<(bool, PetStats)>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let store = pet.store(bar.spells.pet_guid);
    let family = family_for(store, &names, &commands, family_tables.as_deref());
    let fresh = stats_for(store, self_store.iter().next(), tables.as_deref(), family);
    // Push only on change. Happiness moves on its own clock and the frame repaints off UNIT_*
    // events, so a per-frame push would be pure churn — but the DIFF is what makes this cheap,
    // not a timer, so a real change still lands the same frame it arrives.
    if last.as_ref() == Some(&fresh) {
        return;
    }
    // `UNIT_HAPPINESS` — the pet frame's repaint wire for the icon (ref `PetFrame_OnEvent`'s last
    // arm). The reference fires it off the happiness FIELD changing; we fire it off the three
    // values the icon and its tooltip are built from, which is the same event for every consumer
    // there is — the icon is bucketed and both tooltip lines are bucket-derived, so movement
    // *within* a bucket has nothing to repaint. Named because it is a narrower trigger, not a
    // wider one: nothing reads the raw happiness number.
    let happiness_moved = last.as_ref().map(|(_, s)| {
        (
            s.happiness,
            s.damage_percentage.to_bits(),
            s.loyalty_rate.to_bits(),
        )
    }) != Some((
        fresh.1.happiness,
        fresh.1.damage_percentage.to_bits(),
        fresh.1.loyalty_rate.to_bits(),
    ));
    // The pet page's other two repaint wires (ref `PetPaperDollFrame.lua:9,21`), cut the same
    // narrow way: `UNIT_PET_EXPERIENCE` is the XP bar's ONLY wire (`PetExpBar_Update`, l.43-44 —
    // the one event arm that repaints nothing else), and `UNIT_PET_TRAINING_POINTS` reaches only
    // `PetPaperDollFrame_Update`'s training-point text. Firing each off its own pair rather than
    // off the whole block keeps happiness drift — which moves every few seconds while a pet is
    // out — from repainting two numbers that did not change.
    let xp_moved = last.as_ref().map(|(_, s)| s.experience) != Some(fresh.1.experience);
    let training_moved =
        last.as_ref().map(|(_, s)| s.training_points) != Some(fresh.1.training_points);
    if happiness_moved {
        debug!(
            "ui_pet_stats: happiness {:?} ({}% damage), loyalty {:?}",
            fresh.1.happiness, fresh.1.damage_percentage, fresh.1.loyalty
        );
    }
    // **The family lands LATE and fires NOTHING — a named hole, not an oversight** (decision
    // 1062). It arrives with the creature-query answer, a round trip after the pet streams, and
    // no event the pet page registers is guaranteed to fire then: `fire_transitions` has no arm
    // for it, `UNIT_CLASSIFICATION_CHANGED` (the reference's own "a creature query landed" wire,
    // decision 0782) is neither registered by the page nor even edge-able for a pet, whose gated
    // rank is pinned at 0. Inventing a fire site — `PET_UI_UPDATE`, or `UNIT_LEVEL` because the
    // family shares the level's line — would be asserting a mechanism nobody has found in the
    // reference.
    //
    // What makes it *survivable* rather than broken: the query goes out the frame the pet's
    // descriptor arrives, thousands of frames before a human can open the character window, so
    // the page's own `OnShow` → `_Update` covers every ordinary open. The one reachable blank is
    // a page already sitting open at the instant a pet is summoned, which the next stat event
    // (`UNIT_STATS`/`UNIT_DAMAGE`/…, all of which the page registers and all of which fire
    // repeatedly while a pet's descriptor streams) repaints within a frame or two. This line is
    // the instrument for it: a family that never appears names itself here.
    if last.as_ref().map(|(_, s)| &s.family) != Some(&fresh.1.family) {
        debug!(
            "ui_pet_stats: pet family {:?}, diet {:?}",
            fresh.1.family, fresh.1.food_types
        );
    }
    *last = Some(fresh.clone());
    script.set_pet_stats(fresh.0, fresh.1);
    // Push before firing — dispatch runs the Lua handlers synchronously (the `ui_unit` rule).
    if happiness_moved {
        // `%s` — the unit token, per the reference's own fire site (SignalEvent2).
        // Every 1.12 `UNIT_*` event carries it, and its consumers gate on it: a handler's first
        // line is `if ( arg1 == this.unit )`, so an argless fire reaches nobody.
        script.fire_event("UNIT_HAPPINESS", vec![ScriptValue::Str("pet".into())]);
    }
    // Both of these are the SAME bridge as `UNIT_HAPPINESS` above — named unit-descriptor fields
    // (141/142 `PET_EXPERIENCE`/`PET_NEXT_LEVEL_EXP`, 149 `TRAINING_POINTS`), whose events the
    // reference dispatches through `0x515e50`'s token fan-out, `SignalEvent2(id, "%s", token)`.
    // They were argless here for months, two lines under a comment stating the law, and the cost
    // was not theoretical: `PetPaperDollFrame_OnEvent` gives `UNIT_PET_EXPERIENCE` a named branch
    // but routes `UNIT_PET_TRAINING_POINTS` to its final `elseif ( arg1 == "pet" )` catch-all
    // (`PetPaperDollFrame.lua:44`), so an argless fire reached nobody and the pet page's training
    // points never repainted off the event at all.
    if xp_moved {
        script.fire_event("UNIT_PET_EXPERIENCE", vec![ScriptValue::Str("pet".into())]);
    }
    if training_moved {
        script.fire_event(
            "UNIT_PET_TRAINING_POINTS",
            vec![ScriptValue::Str("pet".into())],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::ObjectFields;

    const BYTES_0: u16 = 36;
    const BYTES_1: u16 = 138;
    const POWER5: u16 = 27;
    const PETNUMBER: u16 = 139;
    const PETXP: u16 = 141;
    const PETNEXTXP: u16 = 142;
    const TRAINING: u16 = 149;
    /// `OBJECT_FIELD_ENTRY` — the pet's creature-template id ([`family_for`]'s whole key).
    const ENTRY: u16 = 3;
    /// Real `creature_template` entries on the live VM, so the family ids below are the ones the
    /// server would actually send: Imp → `pet_family` 23, Stonetusk Boar → 5.
    const IMP_ENTRY: u32 = 416;
    const BOAR_ENTRY: u32 = 113;

    fn hunter() -> ObjectStore {
        // BYTES_0 byte 1 = class. Hunter = 3.
        ObjectStore(ObjectFields::from_pairs(&[(
            BYTES_0,
            u32::from(CLASS_HUNTER) << 8,
        )]))
    }

    fn warlock() -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[(BYTES_0, 9 << 8)]))
    }

    /// A boar: pet number set, loyalty level 6, happy, part-trained, mid-XP, and its real
    /// `creature_template` entry in `OBJECT_FIELD_ENTRY`.
    fn boar() -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (ENTRY, BOAR_ENTRY),
            (PETNUMBER, 42),
            (BYTES_1, 6 << 8),             // loyalty level in byte 1
            (POWER5, 1_000_000),           // maximum happiness
            (TRAINING, (170 << 16) | 130), // total in the HIGH word, spent in the low
            (PETXP, 4200),
            (PETNEXTXP, 8000),
        ]))
    }

    fn chain() -> Option<benilla_formats::Chain> {
        let data = benilla_formats::wow_data_or_skip!(None);
        Some(benilla_formats::open_chain(&data).expect("open chain"))
    }

    fn tables() -> Option<PetStatTables> {
        let mut chain = chain()?;
        Some(PetStatTables {
            personalities: benilla_formats::load_pet_personalities(&mut chain)
                .expect("personality"),
            loyalty: benilla_formats::load_pet_loyalty_names(&mut chain).expect("loyalty"),
        })
    }

    fn family_tables() -> Option<PetFamilyTables> {
        let mut chain = chain()?;
        Some(PetFamilyTables {
            families: benilla_formats::load_creature_families(&mut chain).expect("families"),
            foods: benilla_formats::load_pet_food_names(&mut chain).expect("foods"),
        })
    }

    /// The "no family resolved" pair, for the stat tests that are not about the family.
    fn no_family() -> (Option<String>, Option<String>, Vec<String>) {
        (None, None, Vec::new())
    }

    /// A `NameCache` with `entry`'s creature record already cached, carrying `pet_family` —
    /// i.e. the state after `SMSG_CREATURE_QUERY_RESPONSE` has landed.
    fn cache_with(entry: u32, pet_family: u32) -> NameCache {
        let mut names = NameCache::default();
        names.insert_creature(
            entry,
            Some(crate::names::CreatureRecord {
                name: "Snarl".into(),
                subname: None,
                creature_type: 1,
                pet_family,
                rank: 0,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
        names
    }

    fn commands() -> (
        NetCommands,
        crossbeam_channel::Receiver<crate::net::ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    /// The whole block end to end on the real DBC data — the packed/derived reads are the point:
    /// the training-points word order, the loyalty byte, and happiness coming out PRE-BUCKETED
    /// with vanilla's own 125% damage for a maxed pet.
    #[test]
    fn a_hunters_boar_reads_every_field() {
        let Some(t) = tables() else { return };
        let (has_ui, s) = stats_for(Some(&boar()), Some(&hunter()), Some(&t), no_family());
        assert!(has_ui && s.hunter_pet);
        assert_eq!(s.happiness, Some(3));
        assert_eq!(s.damage_percentage, 125.0);
        assert_eq!(s.loyalty_rate, 20.0);
        assert_eq!(s.loyalty.as_deref(), Some("(Loyalty Level 6) Best Friend"));
        assert_eq!(
            s.training_points,
            (170, 130),
            "TOTAL first, from the high word"
        );
        assert_eq!(s.experience, (4200, 8000));
    }

    /// The happiness bucket really is derived from the power field, not passed through: three
    /// different raw values give three different buckets and three different damage numbers.
    #[test]
    fn happiness_buckets_the_raw_power() {
        let Some(t) = tables() else { return };
        let at = |raw: u32| {
            let store = ObjectStore(ObjectFields::from_pairs(&[
                (PETNUMBER, 42),
                (BYTES_1, 0),
                (POWER5, raw),
            ]));
            let (_, s) = stats_for(Some(&store), Some(&hunter()), Some(&t), no_family());
            (s.happiness, s.damage_percentage)
        };
        assert_eq!(at(0), (Some(1), 75.0), "an unhappy pet deals 75%");
        assert_eq!(at(500_000), (Some(2), 100.0));
        assert_eq!(at(1_000_000), (Some(3), 125.0));
    }

    /// **The gate is the class, and it takes everything with it.** A warlock's imp has a pet
    /// number — so `HasPetUI`'s first return stays true — and answers no stats at all. Letting one
    /// value leak past the gate is the failure this guards: an imp with a loyalty level.
    #[test]
    fn a_warlocks_minion_has_a_ui_and_no_stats() {
        let Some(t) = tables() else { return };
        let (has_ui, s) = stats_for(Some(&boar()), Some(&warlock()), Some(&t), no_family());
        assert!(
            has_ui,
            "the pet number is what HasPetUI's first return reads"
        );
        assert!(!s.hunter_pet);
        assert_eq!(s.happiness, None);
        assert_eq!(s.loyalty, None);
        assert_eq!(s.training_points, (0, 0));
        assert_eq!(s.experience, (0, 0));
    }

    /// A possessed creature has an action bar but no pet number, so it gets no paper doll either —
    /// the two gates are genuinely different (`PetHasActionBar` is the cached guid alone).
    #[test]
    fn no_pet_number_means_no_pet_ui() {
        let Some(t) = tables() else { return };
        let possessed = ObjectStore(ObjectFields::from_pairs(&[(POWER5, 1_000_000)]));
        let (has_ui, s) = stats_for(Some(&possessed), Some(&hunter()), Some(&t), no_family());
        assert!(!has_ui);
        assert!(!s.hunter_pet);
    }

    /// Loyalty level 0 is nil, not level 1 — the client's own bound, and the difference between a
    /// fresh pet showing nothing and showing "Rebellious".
    #[test]
    fn loyalty_level_zero_is_nil() {
        let Some(t) = tables() else { return };
        let fresh = ObjectStore(ObjectFields::from_pairs(&[
            (PETNUMBER, 42),
            (BYTES_1, 0),
            (POWER5, 1_000_000),
        ]));
        let (_, s) = stats_for(Some(&fresh), Some(&hunter()), Some(&t), no_family());
        assert_eq!(s.loyalty, None);
        assert_eq!(s.happiness, Some(3), "…but happiness still answers");
    }

    /// Missing client data degrades to the bindings' own gate-failure numbers rather than to a
    /// wrong bucket: nil happiness with `(100.0, 0.0)` beside it.
    #[test]
    fn absent_dbc_data_degrades_to_the_failure_numbers() {
        let (has_ui, s) = stats_for(Some(&boar()), Some(&hunter()), None, no_family());
        assert!(has_ui && s.hunter_pet);
        assert_eq!(s.happiness, None);
        assert_eq!((s.damage_percentage, s.loyalty_rate), (100.0, 0.0));
        // The descriptor-only values still read — they need no table.
        assert_eq!(s.training_points, (170, 130));
    }

    /// The RUNTIME leg on the real data: every GlobalStrings key the happiness tooltip resolves
    /// exists in the shipped 1.12 file, with the `%d` the damage line formats into. A typo'd key
    /// here is silent — `getglobal` answers nil and the tooltip simply loses a line.
    #[test]
    fn every_happiness_string_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| {
            s.lua()
                .globals()
                .get::<String>(key)
                .ok()
                .unwrap_or_default()
        };

        // The three bucket names are resolved by CONCATENATION (`"PET_HAPPINESS"..happiness`), so
        // a missing one shows as an empty tooltip for that bucket alone — exactly the failure a
        // by-eye check misses.
        assert_eq!(g("PET_HAPPINESS1"), "Unhappy");
        assert_eq!(g("PET_HAPPINESS2"), "Content");
        assert_eq!(g("PET_HAPPINESS3"), "Happy");
        assert!(
            g("PET_DAMAGE_PERCENTAGE").contains("%d"),
            "the damage line formats the percentage in: {:?}",
            g("PET_DAMAGE_PERCENTAGE")
        );
        assert!(!g("LOSING_LOYALTY").is_empty());
        assert!(!g("GAINING_LOYALTY").is_empty());
    }

    /// No pet at all: no UI, no stats, and nothing that could be mistaken for a real answer.
    #[test]
    fn no_pet_is_no_ui() {
        let (has_ui, s) = stats_for(None, Some(&hunter()), None, no_family());
        assert!(!has_ui);
        assert_eq!(s, PetStats::default());
    }

    /// **The family lookup end to end on the real DBC data**, including all three
    /// nil sources the binding has to reproduce.
    ///
    /// The load-bearing fact under test is the KEY: the pet's template entry is read from
    /// `OBJECT_FIELD_ENTRY`, not from its guid (whose entry-shaped slot holds a pet number). A
    /// version that asked the guid would query a nonexistent template and answer nil forever —
    /// silently, since nil is a legitimate answer here.
    #[test]
    fn the_pets_family_resolves_off_its_descriptor_entry() {
        let Some(t) = family_tables() else { return };
        let (cmds, rx) = commands();

        // 1. No pet at all.
        let names = NameCache::default();
        assert_eq!(family_for(None, &names, &cmds, Some(&t)), no_family());
        assert!(rx.try_recv().is_err(), "nothing to ask about");

        // 2. A pet whose creature query has NOT answered yet — nil, and the ask goes out (once).
        let pet = ObjectStore(ObjectFields::from_pairs(&[
            (ENTRY, IMP_ENTRY),
            (PETNUMBER, 7),
        ]));
        assert_eq!(
            family_for(Some(&pet), &names, &cmds, Some(&t)),
            no_family(),
            "un-queried is nil, not a guess"
        );
        assert!(
            matches!(
                rx.try_recv(),
                Ok(crate::net::ClientCommand::CreatureQuery { entry, .. }) if entry == IMP_ENTRY
            ),
            "the pet's DESCRIPTOR entry is what gets queried"
        );
        assert!(rx.try_recv().is_err(), "ask-once");

        // 3. The answer lands with family 0 — a template with no family. Still nil, and this is
        //    the common case for every non-tameable creature.
        let names = cache_with(IMP_ENTRY, 0);
        assert_eq!(family_for(Some(&pet), &names, &cmds, Some(&t)), no_family());

        // 4. The answer lands with the Imp's real family (23, from the live `creature_template`).
        //    A warlock minion: a word, and an EMPTY diet — mask 0 in the shipped DBC.
        let names = cache_with(IMP_ENTRY, 23);
        assert_eq!(
            family_for(Some(&pet), &names, &cmds, Some(&t)),
            (
                Some("Imp".into()),
                // The family row's own icon column — and the shipped value for a
                // warlock minion is **`Ability_Druid_CatForm`**, not any imp art. That is not a
                // misread: rows 15 (Felhunter) and 23 (Imp) both carry it in the real 5875 file,
                // where every one of the 22 hunter families carries its correct
                // `Ability_Hunter_Pet_<Family>`. It is placeholder data Blizzard never filled in,
                // and nothing in the reference can show it — the stable is the column's only
                // consumer and it early-returns for warlocks before reading one. Asserted as
                // shipped rather than as expected.
                Some("Interface\\Icons\\Ability_Druid_CatForm".into()),
                Vec::new()
            )
        );

        // 5. A hunter's boar (entry 113 → family 5): a word AND the six-diet list, in bit order.
        let names = cache_with(BOAR_ENTRY, 5);
        let (name, icon, diet) = family_for(Some(&boar()), &names, &cmds, Some(&t));
        assert_eq!(name.as_deref(), Some("Boar"));
        assert_eq!(
            icon.as_deref(),
            Some("Interface\\Icons\\Ability_Hunter_Pet_Boar")
        );
        assert_eq!(diet, ["Meat", "Fish", "Cheese", "Bread", "Fungus", "Fruit"]);

        // 6. No DBC tables at all: nil, degraded to exactly the blank level line 1057 shipped.
        let names = cache_with(BOAR_ENTRY, 5);
        assert_eq!(family_for(Some(&boar()), &names, &cmds, None), no_family());
    }

    /// **The family WORD survives the hunter gate; the DIET does not** — the reference's split
    /// (`UnitCreatureFamily 0x51a310` has no class test, `GetPetFoodTypes 0x4bea10` shares
    /// `0x6116e0` with the four stats).
    ///
    /// The pet here is deliberately a **boar under a non-hunter** — a charmed beast, family 5,
    /// food mask 63 — because that is the one case where the gate is observable at all: a warlock
    /// minion's family ships mask 0, so an ungated implementation would look identical for it and
    /// diverge only here. Folding the family word into the gate would blank a level line the
    /// reference fills; leaving the diet out of it would feed a priest's mind-controlled boar a
    /// six-item menu the reference never shows.
    #[test]
    fn a_charmed_beast_keeps_its_family_word_and_loses_its_diet() {
        let Some(t) = tables() else { return };
        let boar_diet: Vec<String> = ["Meat", "Fish", "Cheese", "Bread", "Fungus", "Fruit"]
            .map(String::from)
            .to_vec();
        let (_, s) = stats_for(
            Some(&boar()),
            Some(&warlock()),
            Some(&t),
            (Some("Boar".into()), None, boar_diet.clone()),
        );
        assert!(!s.hunter_pet);
        assert_eq!(s.family.as_deref(), Some("Boar"), "the word is ungated");
        assert!(
            s.food_types.is_empty(),
            "…but the diet shares 0x6116e0 with the stats"
        );
        assert_eq!(s.loyalty, None, "…as does the hunter machinery");
        assert_eq!(s.happiness, None);

        // The same pet under a HUNTER gets both.
        let (_, s) = stats_for(
            Some(&boar()),
            Some(&hunter()),
            Some(&t),
            (Some("Boar".into()), None, boar_diet.clone()),
        );
        assert_eq!(s.family.as_deref(), Some("Boar"));
        assert_eq!(s.food_types, boar_diet);
    }

    /// …and with no pet, the family goes with everything else — a stale word left on the block
    /// would outlive the pet on a page that is about to close.
    #[test]
    fn no_pet_drops_the_family_too() {
        let (_, s) = stats_for(
            None,
            Some(&hunter()),
            None,
            (Some("Imp".into()), None, vec![]),
        );
        assert_eq!(s.family, None);
        assert!(s.food_types.is_empty());
    }

    /// **The schedule invariant, not the function's**: by the time `UNIT_PET`
    /// reaches a Lua handler, `HasPetUI()` must already answer for the pet that event announces.
    ///
    /// This is the Pet-tab bug in its smallest reproducible form, and it is only visible from a
    /// real `App`: every unit test above calls `stats_for` directly and passes whatever the
    /// ordering does. Driven through **one** `app.update()` on a cold pet — the exact live shape,
    /// where `feed_pet_unit` sees the guid appear and fires while `feed_pet_stats` has or has not
    /// yet pushed. A handler that reads `HasPetUI()` at that instant is what the shipped
    /// `PetTab_Update` is, minus the frames.
    #[test]
    fn unit_pet_reaches_lua_with_has_pet_ui_already_true() {
        use crate::char_select::ClientState;
        use crate::net::GuidIndex;
        use crate::ui_pet::UiPetPlugin;

        const PET_GUID: u64 = 0xf140_0000_0000_002a;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::InWorld)
            .init_resource::<NameCache>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::net::FieldChanged>()
            .insert_resource(NetCommands(tx))
            .add_plugins((UiPetStatsPlugin, UiPetPlugin));

        // The pet is already in the world when the bar's guid lands — the cold-summon shape.
        let pet = app.world_mut().spawn(boar()).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(PET_GUID, pet);
        app.world_mut().resource_mut::<PetBar>().spells.pet_guid = PET_GUID;

        // A bare frame standing in for the pet page: it records what `HasPetUI()` answered at the
        // moment the event was dispatched, which is the only thing under test.
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                BENILLA_SAW = "never fired"
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_PET")
                f:SetScript("OnEvent", function()
                    BENILLA_SAW = HasPetUI() and "has pet UI" or "no pet UI"
                end)
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);

        app.update();

        let saw: String = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("return BENILLA_SAW")
            .unwrap();
        assert_eq!(
            saw, "has pet UI",
            "UNIT_PET must not reach Lua ahead of the snapshot its handlers read — unordered, \
             this answered \"no pet UI\" and the Pet tab stayed down for the session"
        );
    }
}
