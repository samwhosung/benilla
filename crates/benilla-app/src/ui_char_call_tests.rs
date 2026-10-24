//! `AbandonSkill` through the whole client's host: it sends `CMSG_UNLEARN_SKILL` inside its call
//! (`0x4d3b40`), so a chunk's packets leave in call order with every other verb's.

use benilla_ui::script::UiScript;
use crossbeam_channel::Receiver;

use super::*;
use crate::script_calls::with_hosted_vm;

/// The whole client, headless, its VM seated with two categories: rows 1 Weapon Skills,
/// 2 Swords, 3 Professions, 4 First Aid.
fn client() -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let entry = |skill_id: u32, name: &str, category_id: u32, category: &str| SkillEntry {
        skill_id,
        name: name.into(),
        value: 10,
        max: 75,
        mono: false,
        temp_bonus: 0,
        perm_bonus: 0,
        min_level: 0,
        cost_index: 0,
        category_id,
        category_name: category.into(),
        category_order: category_id,
        description: String::new(),
        abandonable: true,
        step: 1,
        flags: 0,
        tier_id: 0,
    };
    let mut script = UiScript::new().expect("a VM");
    script.set_skills(SkillsState {
        entries: vec![
            entry(43, "Swords", 1, "Weapon Skills"),
            entry(129, "First Aid", 2, "Professions"),
        ],
    });
    app.world_mut().insert_non_send_resource(script);
    (app, rx)
}

fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| !c.contains("Query"))
        .collect()
}

/// The row's skill line goes out inside the call; a header row's is 0, which the reference sends
/// too, as it tests only that the row exists (`0x4d3b04`); an index past the rows sends nothing
/// (`0x4d3af7`).
#[test]
fn abandon_skill_sends_inside_its_call() {
    let (mut app, rx) = client();
    let _ = sent(&rx);
    with_hosted_vm(app.world_mut(), |vm| {
        vm.expect("a VM")
            .run("AbandonSkill(4) RandomRoll(1, 2) AbandonSkill(3) AbandonSkill(9)")
    })
    .expect("the chunk runs");
    assert_eq!(
        sent(&rx),
        [
            format!("{:?}", ClientCommand::UnlearnSkill { skill_id: 129 }),
            format!("{:?}", ClientCommand::RandomRoll { min: 1, max: 2 }),
            format!("{:?}", ClientCommand::UnlearnSkill { skill_id: 0 }),
        ]
    );
}
