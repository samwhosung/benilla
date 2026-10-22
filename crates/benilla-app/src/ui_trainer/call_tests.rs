//! The trainer verbs through the whole client's host: `BuyTrainerService` (`0x4da210` →
//! `0x4d89d0`) and `OpenTrainer` (`0x4d8cf0` → `0x5df680`) send inside their calls, so a chunk's
//! packets leave in call order with every other verb's.

use benilla_protocol::ObjectFields;
use crossbeam_channel::Receiver;

use super::*;
use crate::script_calls::with_hosted_vm;

const ME: u64 = 0x10;
const TRAINER: u64 = 0xF130_0000_0000_0042;

/// The whole client, headless, with a class trainer open on [`TRAINER`] offering Heroic Strike
/// (78), available, under one Arms header.
fn client() -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let me = world
        .spawn((
            SelfPlayer,
            Guid(ME),
            ObjectStore(ObjectFields::from_pairs(&[(2, 0x19)])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    world.insert_resource(crate::net::SelfGuid(Some(ME)));
    world.resource_mut::<TrainerOpen>().open(
        TRAINER,
        0,
        vec![TrainerSpell {
            spell: 78,
            state: benilla_protocol::messages::trainer_spell_state::GREEN,
            cost: 10,
            can_learn_primary_prof: false,
            is_primary_prof_first_rank: false,
            req_level: 1,
            req_skill: 0,
            req_skill_value: 0,
            req_spells: [0, 0, 0],
        }],
        String::new(),
    );
    world.resource_mut::<TrainerOpen>().fresh_list = false;
    let mut script = UiScript::new().expect("a VM");
    script.set_trainer(Some(TrainerState {
        guid: TRAINER,
        services: vec![TrainerService {
            spell_id: 78,
            name: Some("Heroic Strike".into()),
            subtext: Some("Rank 1".into()),
            group_key: 26,
            group_name: "Arms".into(),
            ..Default::default()
        }],
        greeting: String::new(),
        trainer_type: 0,
        groups: Vec::new(),
    }));
    world.insert_non_send_resource(script);
    (app, rx)
}

fn run(app: &mut App, lua: &str) {
    with_hosted_vm(app.world_mut(), |vm| vm.expect("a VM").run(lua)).expect("the chunk runs");
}

/// The packets a chunk sent, the name queries left out.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| !c.contains("Query"))
        .collect()
}

/// `BuyTrainerService(2)` sends `CMSG_TRAINER_BUY_SPELL` with the trainer's cell
/// (`0x4d8a11`-`0x4d8a26`) and `OpenTrainer()` `CMSG_TRAINER_LIST` with the player's own guid
/// (`0x4d8d3c` → `0x5df680`), each inside its call, so the roll between them leaves between them.
#[test]
fn the_trainer_verbs_send_inside_their_calls() {
    let (mut app, rx) = client();
    let _ = sent(&rx);
    run(
        &mut app,
        "BuyTrainerService(2) RandomRoll(1, 2) OpenTrainer() RandomRoll(3, 4)",
    );
    assert_eq!(
        sent(&rx),
        [
            format!("TrainerBuySpell {{ trainer: {TRAINER}, spell_id: 78 }}"),
            "RandomRoll { min: 1, max: 2 }".to_string(),
            format!("TrainerList {{ trainer: {ME} }}"),
            "RandomRoll { min: 3, max: 4 }".to_string(),
        ]
    );
}
