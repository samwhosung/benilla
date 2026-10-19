use super::fixture::*;
use super::*;
// Over the prelude's `Scene`.
use super::fixture::Scene;

const ME: u64 = 0x10;
const FOE: u64 = 0x11;
const FOE_PET: u64 = 0x12;
const MY_PET: u64 = 0x13;
const MEMBER: u64 = 0x14;
const MEMBER_PET: u64 = 0x15;
const STRANGER: u64 = 0x16;
const OTHER: u64 = 0x17;
const NPC: u64 = 0x18;
const ARBITER: u64 = 0xF000_0000_0000_0001;
const OTHER_ARBITER: u64 = 0xF000_0000_0000_0002;

/// One object toward itself is friendly before any rung (`0x606200`), so the both-FFA rung never
/// makes you hostile to yourself and `CanAttack` refuses (`0x606b03`). The party rung would cover
/// the active player alone; another player toward himself takes the identity rung too.
#[test]
fn you_are_friendly_to_yourself_in_a_free_for_all_area() {
    for flags in [0, PVP] {
        let me = flagged(with_flags(player(ME, HUMAN), flags), FFA);
        let mut scene = Scene::new(me, None);
        let them = scene.hold(flagged(with_flags(player(STRANGER, HUMAN), flags), FFA));
        let rel = scene.rel();
        for unit in [me, them] {
            assert_eq!(rel.reaction(&unit, &unit), 4, "{:#x}", unit.guid);
            assert!(!rel.can_attack(&unit, &unit), "{:#x}", unit.guid);
        }
    }
    // Another FFA player is hostile and attackable.
    let me = flagged(player(ME, HUMAN), FFA);
    let mut scene = Scene::new(me, None);
    let them = scene.hold(flagged(player(STRANGER, HUMAN), FFA));
    let rel = scene.rel();
    assert_eq!(rel.reaction(&them, &me), 1);
    assert!(rel.can_attack(&me, &them));
}

/// The duel rung runs on the controlling players (`0x606247`-`0x6062ad`): two humans in one duel
/// are hostile once both teams are set, and so is the opponent's pet, either way round; `CanAttack`
/// follows on the same players (`0x606b05`-`0x606b43`). During the countdown, with no team, they
/// are the friends their templates make them.
#[test]
fn a_duel_opponents_pet_is_hostile_and_attackable_both_ways() {
    let me = duelling(player(ME, HUMAN), ARBITER, 1);
    let mut scene = Scene::new(me, Some(factions()));
    let foe = scene.hold(duelling(player(FOE, HUMAN), ARBITER, 2));
    let pet = scene.hold(pet_of(FOE_PET, &foe));
    let rel = scene.rel();
    for (a, b) in [(&me, &foe), (&foe, &me), (&me, &pet), (&pet, &me)] {
        assert_eq!(rel.reaction(a, b), 1, "{:#x} toward {:#x}", a.guid, b.guid);
        assert!(rel.can_attack(a, b), "{:#x} attacks {:#x}", a.guid, b.guid);
    }
    // One team: two on two, the partner's pet a friend.
    let partner = duelling(player(FOE, HUMAN), ARBITER, 1);
    let mut scene = Scene::new(me, Some(factions()));
    scene.hold(partner);
    let partner_pet = scene.hold(pet_of(FOE_PET, &partner));
    let rel = scene.rel();
    assert_eq!(rel.reaction(&me, &partner_pet), 4);
    assert!(!rel.can_attack(&me, &partner_pet));
    // The countdown: no team yet, so the faction legs decide, and a human is a friend.
    let counting = |u: RelUnit| duelling(u, ARBITER, 0);
    let mut scene = Scene::new(counting(me), Some(factions()));
    let foe = scene.hold(counting(player(FOE, HUMAN)));
    let pet = scene.hold(pet_of(FOE_PET, &foe));
    let rel = scene.rel();
    assert_eq!(rel.reaction(&counting(me), &pet), 4);
    assert!(!rel.can_attack(&counting(me), &pet));
    // Two duels, two arbiters: no rung.
    let mut scene = Scene::new(me, Some(factions()));
    let elsewhere = scene.hold(duelling(player(FOE, HUMAN), OTHER_ARBITER, 2));
    assert_eq!(scene.rel().reaction(&me, &elsewhere), 4);
    // The rung compares the arbiters and needs none (`0x606288`-`0x606294`): teams set over two
    // zero arbiters engage it.
    let me = duelling(player(ME, HUMAN), 0, 1);
    let mut scene = Scene::new(me, Some(factions()));
    let foe = scene.hold(duelling(player(FOE, HUMAN), 0, 2));
    assert_eq!(scene.rel().reaction(&me, &foe), 1);
}

/// A player-controlled unit whose controlling player the object manager does not hold is neutral
/// to another player-controlled unit (`0x60625d`, `0x60635f`), and `CanAttack` lets a controlled
/// attacker strike it (`0x606b15`-`0x606b1b`), while `CanAssist` refuses at the reaction.
#[test]
fn a_pet_whose_owner_is_not_held_is_neutral_and_attackable() {
    let me = player(ME, HUMAN);
    let mut scene = Scene::new(me, Some(factions()));
    let owner = player(FOE, HUMAN);
    let pet = scene.hold(pet_of(FOE_PET, &owner));
    let rel = scene.rel();
    assert_eq!(rel.reaction(&me, &pet), 3);
    assert_eq!(rel.reaction(&pet, &me), 3);
    assert!(rel.can_attack(&me, &pet));
    assert!(!rel.can_assist(&me, &pet));
    // Held, it is its owner's friend, as its owner is ours.
    scene.hold(owner);
    let rel = scene.rel();
    assert_eq!(rel.reaction(&me, &pet), 4);
    assert!(!rel.can_attack(&me, &pet));
}

/// The party rung (`0x6062b0`-`0x60631c`), between the duel and both-FFA rungs: the active player
/// and a party member are friendly either way round in a free-for-all area, and so are their pets,
/// so neither can attack the other; a stranger in the same arena is hostile. The rung needs the
/// active player on one side (`0x6062bb`, `0x6062f4`), so two other members are hostile to each
/// other there, and a member to his own pet.
#[test]
fn party_members_in_a_free_for_all_area_are_friendly() {
    let ffa = |u: RelUnit| flagged(u, FFA);
    let me = ffa(player(ME, HUMAN));
    let mut scene = Scene::new(me, Some(factions()));
    let member = scene.hold(ffa(player(MEMBER, HUMAN)));
    let member_pet = scene.hold(pet_of(MEMBER_PET, &member));
    let my_pet = scene.hold(pet_of(MY_PET, &me));
    let other = scene.hold(ffa(player(OTHER, HUMAN)));
    let stranger = scene.hold(ffa(player(STRANGER, HUMAN)));
    scene.party(&[MEMBER, OTHER]);
    let rel = scene.rel();
    for (a, b) in [
        (&me, &member),
        (&member, &me),
        (&me, &member_pet),
        (&member_pet, &me),
        (&me, &my_pet),
        (&my_pet, &me),
    ] {
        assert_eq!(rel.reaction(a, b), 4, "{:#x} toward {:#x}", a.guid, b.guid);
        assert!(!rel.can_attack(a, b), "{:#x} attacks {:#x}", a.guid, b.guid);
    }
    assert_eq!(rel.reaction(&me, &stranger), 1);
    assert!(rel.can_attack(&me, &stranger));
    assert_eq!(
        rel.reaction(&member, &other),
        1,
        "neither is the active player"
    );
    assert_eq!(rel.reaction(&member, &member_pet), 1, "nor here");
    // Out of the arena the party rung changes nothing a friend's template had not.
    let mut scene = Scene::new(player(ME, HUMAN), Some(factions()));
    let member = scene.hold(player(MEMBER, HUMAN));
    scene.party(&[MEMBER]);
    assert_eq!(scene.rel().reaction(&player(ME, HUMAN), &member), 4);
}

/// The party rung beats a template: an enemy-faction member, a player another member charmed for
/// instance, is a friend of the active player (`0x60631c` comes before the faction legs), though
/// its template is hostile.
#[test]
fn the_party_rung_comes_before_the_templates() {
    let me = player(ME, HUMAN);
    let mut scene = Scene::new(me, Some(factions()));
    let orc = scene.hold(player(MEMBER, ORC));
    assert_eq!(scene.rel().reaction(&me, &orc), 1, "not in the party");
    scene.party(&[MEMBER]);
    assert_eq!(scene.rel().reaction(&me, &orc), 4);
    assert_eq!(scene.rel().reaction(&orc, &me), 4);
}

/// `0x6065be`-`0x6065d2`: a template flagged `0x1000` is hostile toward any unit the active player
/// controls while that player is contested, ahead of the forced table (`0x6065e6`). It is the
/// template's side only: the player's own reaction toward the guard, and the guard's toward a
/// contested player who is not the active one, keep their faction answers.
#[test]
fn a_contested_guard_turns_on_the_contested_player_and_his_pet() {
    let me = flagged(player(ME, HUMAN), CONTESTED);
    let mut scene = Scene::new(me, Some(factions()));
    let guard = scene.hold(creature(NPC, GUARD));
    let my_pet = scene.hold(pet_of(MY_PET, &me));
    let other = scene.hold(flagged(player(OTHER, HUMAN), CONTESTED));
    scene.reputations.set_forced(vec![(3, 4)]);
    let rel = scene.rel();
    assert_eq!(rel.reaction(&guard, &me), 1);
    assert_eq!(rel.reaction(&guard, &my_pet), 1);
    assert!(rel.can_attack(&guard, &me), "a mixed pair below friendly");
    assert_eq!(
        rel.reaction(&me, &guard),
        4,
        "the player's side is untouched"
    );
    assert_eq!(rel.reaction(&guard, &other), 4, "another contested player");
    // Not contested: the forced table answers.
    let me = player(ME, HUMAN);
    let mut scene = Scene::new(me, Some(factions()));
    scene.reputations.set_forced(vec![(3, 5)]);
    assert_eq!(scene.rel().reaction(&guard, &me), 5);
    // An unflagged template ignores the contested bit.
    let me = flagged(player(ME, HUMAN), CONTESTED);
    let scene = Scene::new(me, Some(factions()));
    assert_eq!(scene.rel().reaction(&creature(NPC, HUMAN), &me), 4);
}

/// `CanAttack(this, b)` is the attacker's question: between players it takes `b`'s controlling
/// player's PvP flag (`0x606b45`), so a flagged orc is attackable by an unflagged human who cannot
/// be attacked back, though each is hostile to the other.
#[test]
fn can_attack_takes_the_attacker_first() {
    let me = player(ME, HUMAN);
    let mut scene = Scene::new(me, Some(factions()));
    let orc = scene.hold(with_flags(player(FOE, ORC), PVP));
    let rel = scene.rel();
    assert_eq!((rel.reaction(&me, &orc), rel.reaction(&orc, &me)), (1, 1));
    assert!(rel.can_attack(&me, &orc));
    assert!(!rel.can_attack(&orc, &me));
    // Both flagged, both ways.
    let me = with_flags(player(ME, HUMAN), PVP);
    let mut scene = Scene::new(me, Some(factions()));
    scene.hold(orc);
    assert!(scene.rel().can_attack(&orc, &me));
}

/// A charmed player answers with its charmer (`0x606170` reads `CHARMEDBY` first): a human that a
/// PvP-flagged orc mind-controls is attackable by its own side, whatever its own flags say, and one
/// an unflagged orc controls is not. `CanCooperate` refuses a charmed side (`0x606bab`-`0x606bc5`).
#[test]
fn a_charmed_player_answers_with_its_charmer() {
    let me = player(ME, HUMAN);
    // vmangos gives the victim its charmer's template (`SpellAuras.cpp:3202-3203`).
    let victim = RelUnit {
        charmed_by: FOE,
        ..with_flags(player(OTHER, ORC), PVP)
    };
    for (charmer_flags, attackable) in [(PVP, true), (0, false)] {
        let mut scene = Scene::new(me, Some(factions()));
        scene.hold(with_flags(player(FOE, ORC), charmer_flags));
        let victim = scene.hold(victim);
        let rel = scene.rel();
        assert_eq!(rel.reaction(&me, &victim), 1);
        assert_eq!(
            rel.can_attack(&me, &victim),
            attackable,
            "charmer flags {charmer_flags:#x}"
        );
        assert!(!rel.can_cooperate(&me, &victim));
    }
    // The charmer not held: no controlling player, neutral and attackable (`0x606b15`).
    let mut scene = Scene::new(me, Some(factions()));
    let victim = scene.hold(victim);
    let rel = scene.rel();
    assert_eq!(rel.reaction(&me, &victim), 3);
    assert!(rel.can_attack(&me, &victim));
}

/// The active player charmed answers with the charmer too: the party rung needs the active player
/// to control one side (`0x6062bb`), so under a free-for-all charmer a party member in the arena
/// is hostile to us.
#[test]
fn a_charmed_active_player_loses_the_party_rung() {
    let ffa = |u: RelUnit| flagged(u, FFA);
    let me = RelUnit {
        charmed_by: FOE,
        ..ffa(player(ME, HUMAN))
    };
    let mut scene = Scene::new(me, Some(factions()));
    scene.hold(ffa(player(FOE, ORC)));
    let member = scene.hold(ffa(player(MEMBER, HUMAN)));
    scene.party(&[MEMBER]);
    assert_eq!(scene.rel().reaction(&me, &member), 1);
    assert_eq!(scene.rel().reaction(&member, &me), 1);
}

/// `CanAssist`'s player-controlled arm (`0x60673e`-`0x6067a4`) reads the helped side's controlling
/// player: duelling, it needs the helper's player in that duel on that team; free-for-all, it needs
/// the helper's flagged too. Either side's player missing passes. The order matters: a duellist may
/// help a bystander.
#[test]
fn can_assist_follows_the_helped_players_duel_and_free_for_all() {
    let me = player(ME, HUMAN);
    let mut scene = Scene::new(me, Some(factions()));
    let duellist = scene.hold(duelling(player(OTHER, HUMAN), ARBITER, 1));
    let duellist_pet = scene.hold(pet_of(MEMBER_PET, &duellist));
    let rel = scene.rel();
    assert!(!rel.can_assist(&me, &duellist), "someone else's duel");
    assert!(!rel.can_assist(&me, &duellist_pet), "and its pet");
    assert!(rel.can_assist(&duellist, &me), "the other way round");
    // On the same team of that duel, a two-on-two.
    let teammate = duelling(me, ARBITER, 1);
    let mut scene = Scene::new(teammate, Some(factions()));
    let duellist = scene.hold(duellist);
    assert!(scene.rel().can_assist(&teammate, &duellist));
    // The same team number in another duel is not that duel.
    let elsewhere = duelling(me, OTHER_ARBITER, 1);
    let mut scene = Scene::new(elsewhere, Some(factions()));
    let duellist = scene.hold(duellist);
    assert!(!scene.rel().can_assist(&elsewhere, &duellist));

    // Free-for-all: the helped side's flag needs the helper's.
    let mut scene = Scene::new(me, Some(factions()));
    let ffa = scene.hold(flagged(player(OTHER, HUMAN), FFA));
    let rel = scene.rel();
    assert!(!rel.can_assist(&me, &ffa));
    assert!(rel.can_assist(&ffa, &me));
    // Both flagged in one party: the party rung keeps the reaction friendly, and the arm passes.
    let flagged_me = flagged(me, FFA);
    let mut scene = Scene::new(flagged_me, Some(factions()));
    let member = scene.hold(flagged(player(MEMBER, HUMAN), FFA));
    scene.party(&[MEMBER]);
    assert!(scene.rel().can_assist(&flagged_me, &member));

    // A creature has no controlling player: a friendly NPC may help a duellist.
    let duelling_me = duelling(me, ARBITER, 1);
    let scene = Scene::new(duelling_me, Some(factions()));
    assert!(scene.rel().can_assist(&creature(NPC, HUMAN), &duelling_me));
}

/// `CanAssist` refuses a not-selectable unit first (`0x606706`), and anything less than friendly
/// (`0x60671e`), whatever the arm below would say. The player assists himself, one object being
/// friendly to itself (`0x606200`), until he is not selectable.
#[test]
fn can_assist_needs_a_selectable_friend() {
    let me = player(ME, HUMAN);
    let mut scene = Scene::new(me, Some(factions()));
    let friend = scene.hold(player(OTHER, HUMAN));
    let hidden = scene.hold(with_flags(player(STRANGER, HUMAN), NOT_SELECTABLE));
    let orc = scene.hold(player(FOE, ORC));
    let rel = scene.rel();
    assert!(rel.can_assist(&me, &friend));
    assert!(!rel.can_assist(&me, &hidden));
    assert!(!rel.can_assist(&me, &orc));
    assert!(rel.can_assist(&me, &me));
    let hidden_me = with_flags(me, NOT_SELECTABLE);
    assert!(!rel.can_assist(&hidden_me, &hidden_me));
}

/// `IsPvP 0x605ff0`: a unit whose charmer or creator the object manager holds as a unit
/// (`0x606022`) takes that owner's PvP flag, and answers no under bit 8 (`0x60602c`); without one,
/// its own. `CanAssist` asks it of a creature for a player-controlled helper (`0x6067bd`).
#[test]
fn is_pvp_reads_the_owner_and_answers_no_under_bit_8() {
    let me = player(ME, HUMAN);
    let flagged_owner = with_flags(player(OTHER, HUMAN), PVP);
    let plain_owner = player(STRANGER, HUMAN);
    let mut scene = Scene::new(me, Some(factions()));
    scene.hold(flagged_owner);
    scene.hold(plain_owner);
    // Guardians that are no player's pet: friendly creatures whose creator is a player.
    let guardian = |owner: &RelUnit, flags| RelUnit {
        created_by: owner.guid,
        ..with_flags(creature(NPC, HUMAN), flags)
    };
    let rel = scene.rel();
    assert!(rel.is_pvp(&guardian(&flagged_owner, 0)), "the owner's flag");
    assert!(!rel.is_pvp(&guardian(&plain_owner, PVP)), "not its own");
    assert!(
        !rel.is_pvp(&guardian(&flagged_owner, IMMUNE_TO_PLAYER)),
        "bit 8 refuses"
    );
    assert!(
        rel.is_pvp(&with_flags(creature(NPC, HUMAN), PVP)),
        "no owner"
    );
    let unheld = RelUnit {
        created_by: FOE,
        ..with_flags(creature(NPC, HUMAN), PVP)
    };
    assert!(rel.is_pvp(&unheld), "an owner not held is no owner");
    // The assist arm: a friendly creature is assistable only PvP by `IsPvP`.
    assert!(rel.can_assist(&me, &guardian(&flagged_owner, 0)));
    assert!(!rel.can_assist(&me, &guardian(&plain_owner, PVP)));
    assert!(!rel.can_assist(&me, &creature(NPC, HUMAN)));
    // An uncontrolled helper skips it (`0x6067b9`).
    assert!(rel.can_assist(&creature(NPC, HUMAN), &creature(NPC + 1, HUMAN)));
}

/// `CanAttack`'s ghost gate (`0x606987`-`0x6069b1`): a ghost player is attackable only by a
/// creature whose record is visible to ghosts; the other refusals stand for it.
#[test]
fn only_a_creature_visible_to_ghosts_attacks_a_ghost() {
    let ghost = flagged(player(ME, HUMAN), GHOST);
    let scene = Scene::new(ghost, Some(factions()));
    let rel = scene.rel();
    let mob = creature(NPC, ORC);
    let spirit_mob = RelUnit {
        ghost_visible: true,
        ..mob
    };
    assert!(!rel.can_attack(&mob, &ghost));
    assert!(rel.can_attack(&spirit_mob, &ghost));
    assert!(rel.can_attack(&mob, &player(ME, HUMAN)), "alive");
}

/// `CanInteract`'s ghost leg (`0x606808`-`0x606821`): a ghost player takes a service only from a
/// creature visible to ghosts (`0x605f70`), so a gossip NPC refuses him and a spirit healer does
/// not. The leg reads a player `a` alone (`0x6067ff`).
#[test]
fn a_ghost_interacts_only_with_a_creature_visible_to_ghosts() {
    let me = player(ME, HUMAN);
    let ghost = flagged(me, GHOST);
    let scene = Scene::new(ghost, Some(factions()));
    let rel = scene.rel();
    let gossip = RelUnit {
        npc_flags: crate::target::cursor_mode::npc_flags::GOSSIP,
        ..creature(NPC, HUMAN)
    };
    let healer = RelUnit {
        ghost_visible: true,
        ..gossip
    };
    assert!(!rel.can_interact(&ghost, &gossip));
    assert!(rel.can_interact(&ghost, &healer));
    assert!(rel.can_interact(&me, &gossip), "alive");
    let not_a_player = flagged(creature(NPC + 1, HUMAN), GHOST);
    assert!(rel.can_interact(&not_a_player, &gossip));
}

/// No relation unit has guid 0: the reference's units are live objects, and the persuaded rung
/// (`0x606449`) compares `UNIT_FIELD_PERSUADED`, 0 for none, with one's guid. So the active
/// player is no unit until its guid is known, though its object has streamed.
#[test]
fn the_active_player_is_no_unit_until_its_guid_is_known() {
    use bevy::ecs::system::RunSystemOnce;

    let mut world = World::new();
    world.init_resource::<Reputations>();
    world.spawn((SelfPlayer, store_of(&player(ME, HUMAN))));
    let me = |world: &mut World| {
        world
            .run_system_once(|w: RelationWorld| w.me())
            .expect("run")
    };
    assert_eq!(me(&mut world), None);
    world.insert_resource(SelfGuid(Some(ME)));
    assert_eq!(me(&mut world).map(|u| u.guid), Some(ME));
}

/// `CanAttack`'s flag refusals (`0x6069b7`-`0x6069ff`) and cross immunities (`0x606a05`-
/// `0x606a8a`) beat a hostile reaction; two uncontrolled units fight when either side is hostile
/// (`0x606aab`, `0x606ab8`).
#[test]
fn the_flag_refusals_beat_a_hostile_reaction() {
    let me = player(ME, HUMAN);
    let scene = Scene::new(me, Some(factions()));
    let rel = scene.rel();
    let mob = |flags| with_flags(creature(NPC, ORC), flags);
    assert!(rel.can_attack(&me, &mob(0)));
    for bit in [0x2u32, 0x80, 0x1_0000, 0x10_0000, NOT_SELECTABLE] {
        assert!(!rel.can_attack(&me, &mob(bit)), "victim bit {bit:#x}");
    }
    assert!(
        !rel.can_attack(&me, &mob(0x100)),
        "immune to a controlled attacker"
    );
    assert!(rel.can_attack(&me, &mob(0x200)), "immune only to creatures");
    assert!(
        !rel.can_attack(&mob(0), &with_flags(me, 0x200)),
        "a victim immune to creatures"
    );
    assert!(
        !rel.can_attack(&with_flags(me, 0x200), &mob(0)),
        "the attacker's bit 9"
    );
    assert!(!rel.can_attack(&mob(0x100), &me), "the attacker's bit 8");
    // Two creatures: either reaction hostile is enough.
    let townsman = |guid| creature(guid, HUMAN);
    assert!(rel.can_attack(&townsman(NPC + 1), &mob(0)));
    assert!(rel.can_attack(&mob(0), &townsman(NPC + 1)));
    assert!(!rel.can_attack(&townsman(NPC + 1), &townsman(NPC + 2)));
}

/// `CanCooperate 0x606ba0`: one object refuses (`0x606ba6`), so does a charmed unit on either side
/// (`0x606bab`-`0x606bc5`), and otherwise the faction-group masks decide, with no reaction.
#[test]
fn cooperation_is_the_group_mask() {
    let me = player(ME, HUMAN);
    let scene = Scene::new(me, Some(factions()));
    let rel = scene.rel();
    assert!(!rel.can_cooperate(&me, &me));
    assert!(rel.can_cooperate(&me, &player(OTHER, HUMAN)));
    assert!(rel.can_cooperate(&me, &creature(NPC, GUARD)), "one mask");
    assert!(!rel.can_cooperate(&me, &player(OTHER, ORC)));
    // A duel or a free-for-all moves no mask.
    let ffa = flagged(duelling(player(OTHER, HUMAN), ARBITER, 2), FFA);
    assert!(rel.can_cooperate(&duelling(me, ARBITER, 1), &ffa));
    // A charmed player keeps his mask and still cooperates with nobody, either way round.
    let charmed = RelUnit {
        charmed_by: STRANGER,
        ..player(OTHER, HUMAN)
    };
    assert!(!rel.can_cooperate(&me, &charmed));
    assert!(!rel.can_cooperate(&charmed, &me));
}

/// The shipped tables, through a human warrior's descriptor. The reaction is ordered: toward a
/// reputation faction the active player answers by the at-war bit (`0x606372`-`0x606423`), the
/// faction's unit toward the player by the standing (`0x606609`), so a not-at-war neutral faction
/// is a friend to the player while its standing reads neutral toward him.
mod shipped {
    use super::*;
    use crate::target::stormwind_fixture;

    fn shipped_factions() -> Option<Factions> {
        let data = benilla_formats::wow_data_or_skip!(None);
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        Some(Factions::from_catalog(
            benilla_formats::load_faction_catalog(&mut chain).expect("faction tables"),
        ))
    }

    /// Cenarion Circle, faction 609 in reputation slot 36, template 996: not at war, the player's
    /// reaction is friendly and its unit's toward him neutral; at war, hostile from the player and
    /// still neutral back. `CanAttack` from the player reads the first, so the sword appears only at
    /// war; a gossip NPC is interactable only while both reactions are neutral or better.
    #[test]
    fn a_not_at_war_reputation_faction_is_friendly_from_the_player_and_neutral_toward_him() {
        let Some(factions) = shipped_factions() else {
            return;
        };
        let me = human_warrior(ME);
        let mut scene = Scene::new(me, Some(factions));
        let silent = scene.hold(creature(NPC, 996));
        let gossip = scene.hold(RelUnit {
            npc_flags: crate::target::cursor_mode::npc_flags::GOSSIP,
            ..creature(NPC + 1, 996)
        });
        let stranger = scene.hold(human_warrior(STRANGER));
        let rel = scene.rel();
        assert_eq!(rel.reaction(&me, &silent), 4);
        assert_eq!(rel.reaction(&silent, &me), 3);
        // Another player answers by his template, not by our reputation (`0x606372`-`0x6063a6`).
        let by_template = rel.template_reaction(stranger.template, &silent);
        assert_eq!(rel.reaction(&stranger, &silent), by_template);
        assert!(!rel.can_attack(&me, &silent));
        assert!(!rel.can_interact(&me, &silent), "no service bit");
        assert!(rel.can_interact(&me, &gossip));
        let mut slots = vec![(0u8, 0i32); 64];
        slots[36] = (benilla_formats::faction_flags::AT_WAR, 0);
        scene.reputations = Reputations::from_slots(slots);
        let rel = scene.rel();
        assert_eq!(rel.reaction(&me, &silent), 1);
        assert_eq!(rel.reaction(&silent, &me), 3, "at war moves no standing");
        assert_eq!(
            rel.reaction(&stranger, &silent),
            by_template,
            "our war is not his"
        );
        assert_ne!(by_template, 1);
        assert!(rel.can_attack(&me, &silent));
        assert!(!rel.can_interact(&me, &gossip));
        // A chicken, template 31 with no reputation slot: the comparator, neutral and attackable.
        let chicken = creature(NPC + 2, 31);
        assert_eq!(rel.reaction(&me, &chicken), 3);
        assert!(rel.can_attack(&me, &chicken));
    }

    /// The forced table answers ahead of the reputation legs in both directions (`0x6063e0`,
    /// `0x6065e6`): the player's leg returns it uncapped (`0x6063e9`), the unit's is capped at 6 by
    /// its caller (`0x606439`). Booty Bay, template 120 in slot 1, at war, reads the forced 4 both
    /// ways.
    #[test]
    fn a_forced_rank_answers_both_directions_capped_only_toward_the_player() {
        let Some(factions) = shipped_factions() else {
            return;
        };
        let me = human_warrior(ME);
        let goblin = creature(NPC, 120);
        let faction = factions
            .catalog()
            .template(120)
            .expect("template 120")
            .faction;
        let mut scene = Scene::new(me, Some(factions));
        let mut slots = vec![(0u8, 0i32); 64];
        slots[1] = (benilla_formats::faction_flags::AT_WAR, 0);
        scene.reputations = Reputations::from_slots(slots.clone());
        assert_eq!(scene.rel().reaction(&me, &goblin), 1, "at war");
        scene.reputations.set_forced(vec![(faction, 4)]);
        assert_eq!(scene.rel().reaction(&me, &goblin), 4);
        assert_eq!(scene.rel().reaction(&goblin, &me), 4);
        // A chicken, faction 28 with no slot: a forced 7 reads 6 toward the player, 7 from him.
        let chicken = creature(NPC + 1, 31);
        for (stored, from_player) in [(6, 6), (7, 7), (200, 7)] {
            scene.reputations.set_forced(vec![(28, stored)]);
            let rel = scene.rel();
            assert_eq!(rel.reaction(&chicken, &me), stored.min(6) as u8);
            assert_eq!(rel.reaction(&me, &chicken), from_player);
        }
    }

    /// The standing leg answers only toward a unit the active player controls (`0x6065b0`): a
    /// Stormwind unit reads our rank toward us and toward our pet, and the template comparator
    /// toward another human. The rank stops at Revered (`0x606439`).
    #[test]
    fn the_standing_leg_is_the_active_players_and_capped_at_revered() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let me = human_warrior(ME);
        for (total, want) in [(8_999, 4), (9_000, 5), (21_000, 6), (42_000, 6)] {
            let (factions, template, reps) = stormwind_fixture(&mut chain, total);
            let mut scene = Scene::new(me, Some(factions));
            scene.reputations = reps;
            let pet = scene.hold(pet_of(MY_PET, &me));
            let stranger = scene.hold(human_warrior(STRANGER));
            let guard = creature(NPC, template);
            let rel = scene.rel();
            assert_eq!(rel.reaction(&guard, &me), want, "standing {total}");
            assert_eq!(rel.reaction(&guard, &pet), want, "our pet, {total}");
            assert_eq!(
                rel.reaction(&guard, &stranger),
                4,
                "another human: the comparator"
            );
        }
    }

    /// `0x606449`-`0x6064aa`: a unit persuaded by `b`, or a `b` carrying bit 14, reads a rank more
    /// toward `b` for a template whose faction has a reputation slot, from 2, 3 or 4 only.
    #[test]
    fn a_persuaded_unit_reads_a_rank_friendlier() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let me = human_warrior(ME);
        for (total, plain, raised) in [(-1, 2, 3), (0, 3, 4), (3_000, 4, 5), (9_000, 5, 5)] {
            let (factions, template, reps) = stormwind_fixture(&mut chain, total);
            let mut scene = Scene::new(me, Some(factions));
            scene.reputations = reps;
            let rel = scene.rel();
            let guard = creature(NPC, template);
            let persuaded = RelUnit {
                persuaded: ME,
                ..guard
            };
            assert_eq!(rel.reaction(&guard, &me), plain);
            assert_eq!(rel.reaction(&persuaded, &me), raised, "standing {total}");
            assert_eq!(
                rel.reaction(&guard, &with_flags(me, 0x4000)),
                raised,
                "bit 14, {total}"
            );
        }
        // A template with no reputation faction does not rise.
        let scene = Scene::new(me, shipped_factions());
        let chicken = RelUnit {
            persuaded: ME,
            ..creature(NPC, 31)
        };
        assert_eq!(scene.rel().reaction(&chicken, &me), 3);
    }
}
