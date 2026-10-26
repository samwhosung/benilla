//! The pet verbs through the whole client's host: each sends its packet and changes the bar inside
//! its call, as the reference's bindings do (`0x4be290` → `0x4bcbb0`, `0x4be180` → `0x4bc9a0`,
//! `0x4bd650`, `0x4bd740`, `0x4bd6e0`, `0x4bd840`, `0x4b4240` → `0x4bccb0`), so a chunk's packets go
//! out in call order and its next line reads what the verb did.

use benilla_protocol::messages::{PetActionEntry, PET_ACT_COMMAND, PET_ACT_ENABLED};

use crate::net::{ClientCommand, GuidIndex};

use super::press_tests::{claw, owned_pet_with, rig, Rig, CLAW, FLAGS, PET};
use super::PetBar;

const CLAW_WORD: u32 = CLAW | ((PET_ACT_ENABLED as u32) << 24);
const ATTACK_WORD: u32 = 0x0000_0002 | ((PET_ACT_COMMAND as u32) << 24);
/// `UNIT_FLAG_PET_RENAME`, which `0x4bd8b5` tests.
const PET_RENAME: u32 = 0x10;

/// Our usable pet, renameable, with Claw (autocast on) in slot 1 and in its book, Attack in slot
/// 2, and a VM that records `PET_BAR_UPDATE`, `PET_DISMISS_START` and the red line.
fn scene() -> Rig {
    let mut rig = rig(
        vec![(CLAW, claw())],
        &[CLAW],
        Some(owned_pet_with(&[(22, 100), (28, 100), (FLAGS, PET_RENAME)])),
    );
    {
        let mut bar = rig.app.world_mut().resource_mut::<PetBar>();
        bar.spells.spells = vec![PetActionEntry::from(CLAW_WORD)];
        bar.spells.bar[1] = PetActionEntry::from(ATTACK_WORD);
    }
    ui(&mut rig, |s| {
        s.run(
            r#"HEARD = ""
            ERR_PET_NOT_RENAMEABLE = "Not renameable."
            ERR_NULL_PETNAME = "No name."
            local f = CreateFrame("Frame")
            for _, e in ipairs({"PET_DISMISS_START", "UI_ERROR_MESSAGE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                HEARD = HEARD .. event .. "(" .. tostring(arg1) .. ") "
            end)"#,
        )
        .unwrap()
    });
    rig.frame();
    let _ = rig.sent();
    rig
}

/// `f` on the VM with the call host lent, as every production entry lends it.
fn ui<R>(rig: &mut Rig, f: impl FnOnce(&mut benilla_ui::script::UiScript) -> R) -> R {
    crate::script_calls::with_hosted_vm(rig.app.world_mut(), |s| f(s.expect("a VM")))
}

fn lines(commands: Vec<ClientCommand>) -> Vec<String> {
    commands.iter().map(|c| format!("{c:?}")).collect()
}

/// A chunk with the call host lent, as every entry the stock bar comes in by lends it.
fn run(rig: &mut Rig, lua: &str) {
    crate::script_calls::with_hosted_vm(rig.app.world_mut(), |s| {
        s.expect("a VM").run(lua).expect("the chunk runs");
    });
}

fn heard(rig: &mut Rig) -> String {
    ui(rig, |s| {
        let h = s.eval::<String>("return HEARD").unwrap();
        s.run("HEARD = \"\"").unwrap();
        h
    })
}

/// The pet's packets the chunk sent, by name, in order, nothing drained.
fn pet_packets(rig: &Rig) -> Vec<String> {
    rig.sent()
        .into_iter()
        .filter_map(|c| {
            let name = match c {
                ClientCommand::PetSetAction { .. } => "PetSetAction",
                ClientCommand::PetSpellAutocast { .. } => "PetSpellAutocast",
                ClientCommand::PetStopAttack { .. } => "PetStopAttack",
                ClientCommand::PetRename { .. } => "PetRename",
                ClientCommand::PetAbandon { .. } => "PetAbandon",
                ClientCommand::PetAction { .. } => "PetAction",
                _ => return None,
            };
            Some(name.to_string())
        })
        .collect()
}

/// Every pet verb's packet is out when its call returns, in call order, before any drain runs.
#[test]
fn the_pet_verbs_send_inside_their_calls() {
    let mut rig = scene();
    run(
        &mut rig,
        "PetRename('Rex') TogglePetAutocast(1) ToggleSpellAutocast(1, BOOKTYPE_PET) \
         PickupPetAction(1) PickupPetAction(3) PetAbandon() PetDismiss()",
    );
    assert_eq!(
        pet_packets(&rig),
        [
            "PetRename",
            "PetSetAction",
            "PetSpellAutocast",
            // The pickup's blank of slot 1, then the drop on slot 3 (`0x4be27f`, `0x4bce33`).
            "PetSetAction",
            "PetSetAction",
            "PetAbandon",
            "PetAction",
        ]
    );
}

/// `0x4bccb0`'s send carries the flipped word, and `0x4bcbb0`'s the slot and its word; the bar the
/// app holds moves with them.
#[test]
fn the_autocast_toggles_send_what_they_flipped() {
    let mut rig = scene();
    run(&mut rig, "TogglePetAutocast(1)");
    let off = CLAW_WORD & !0x4000_0000;
    assert_eq!(
        lines(rig.sent()),
        lines(vec![ClientCommand::PetSetAction {
            pet_guid: PET,
            entries: vec![(0, off)],
        }])
    );
    assert_eq!(
        rig.app.world().resource::<PetBar>().spells.bar[0].packed,
        off
    );
    run(&mut rig, "ToggleSpellAutocast(1, BOOKTYPE_PET)");
    assert_eq!(
        lines(rig.sent()),
        lines(vec![ClientCommand::PetSpellAutocast {
            pet_guid: PET,
            spell_id: CLAW,
            enabled: true,
        }])
    );
}

/// `PetDismiss` presses the Dismiss word at the selection (`0x4bd701`, `0x4bd212`), then signals
/// `PET_DISMISS_START` with 10000 (`0x4bd713`-`0x4bd722`), whether or not the press sent.
#[test]
fn pet_dismiss_presses_and_signals_its_start_in_the_verb() {
    let mut rig = scene();
    run(&mut rig, "PetDismiss()");
    assert_eq!(
        lines(rig.sent()),
        lines(vec![ClientCommand::PetAction {
            pet_guid: PET,
            packed: 0x0700_0003,
            target_guid: 0,
        }])
    );
    assert_eq!(heard(&mut rig), "PET_DISMISS_START(10000) ");
}

/// `0x4bd840`'s gates raise their lines inside the call: a pet that cannot be renamed
/// (`0x4bd8b5`, 245) and no name (`0x4bd8d2`, 246) each send nothing.
#[test]
fn pet_rename_raises_its_refusals_in_the_call() {
    let mut rig = scene();
    run(&mut rig, "PetRename()");
    assert_eq!(heard(&mut rig), "UI_ERROR_MESSAGE(No name.) ");
    assert!(pet_packets(&rig).is_empty());
    let pet = rig.app.world().resource::<GuidIndex>().0[&PET];
    rig.app
        .world_mut()
        .entity_mut(pet)
        .insert(owned_pet_with(&[(22, 100), (28, 100)]));
    run(&mut rig, "PetRename('Rex')");
    assert_eq!(heard(&mut rig), "UI_ERROR_MESSAGE(Not renameable.) ");
    assert!(pet_packets(&rig).is_empty());
}

/// `PetStopAttack` with the latch up sends, lowers it and signals `PET_BAR_UPDATE` in the verb
/// (`0x4bd662`-`0x4bd6b4`), so the handler reads Attack unlit.
#[test]
fn pet_stop_attack_sends_and_repaints_in_its_call() {
    let mut rig = scene();
    rig.app
        .world_mut()
        .resource_mut::<super::PetAttackLatch>()
        .0 = Some(PET);
    rig.frame();
    assert!(ui(&mut rig, |s| s
        .eval::<bool>("return IsPetAttackActive(2)")
        .unwrap()));
    ui(&mut rig, |s| {
        s.run(
            "ATTACKING = nil local f = CreateFrame('Frame') f:RegisterEvent('PET_BAR_UPDATE') \
             f:SetScript('OnEvent', function() ATTACKING = IsPetAttackActive(2) end)",
        )
        .unwrap()
    });
    run(&mut rig, "PetStopAttack()");
    assert_eq!(
        lines(rig.sent()),
        lines(vec![ClientCommand::PetStopAttack { pet_guid: PET }])
    );
    assert!(!rig.app.world().resource::<super::PetAttackLatch>().up());
    assert_eq!(
        ui(&mut rig, |s| s
            .eval::<Option<bool>>("return ATTACKING")
            .unwrap()),
        Some(false),
        "the handler ran in the verb and read the latch down"
    );
}
