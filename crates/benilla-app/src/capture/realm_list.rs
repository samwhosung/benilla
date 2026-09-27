//! `glue-realmlist`: the realm list over the login screen with realms in two categories, which the
//! live server cannot show (it lists one). The list is published as the realm park publishes one,
//! then the dialog is raised as Change Realm raises it; the tabs are named from the install's own
//! Region, so bytes 1 and 2 are "English" and "German" under Region 3 and 1 alone under Region 1.

use bevy::prelude::*;

use benilla_protocol::RealmInfo;

use super::CaptureCtx;

/// The scenario's name in [`super::scenarios::GLUE_SCENARIOS`].
pub(super) const NAME: &str = "glue-realmlist";

/// Invented realms: three on category byte 1, two on byte 2, a mix of types, loads and counts.
fn fixture() -> Vec<RealmInfo> {
    let realm =
        |name: &str, category: u8, realm_type: u32, population: f32, characters: u8| RealmInfo {
            name: name.into(),
            address: "127.0.0.1:8085".into(),
            population,
            characters,
            realm_type,
            flags: 0,
            category,
            id: 0,
        };
    vec![
        realm("Amberfield", 1, 0, 0.5, 2),
        realm("Brightwater", 1, 1, 1.5, 0),
        realm("Copperdale", 1, 6, 1.0, 0),
        realm("Dunmoor", 2, 1, 0.8, 0),
        realm("Eisenwald", 2, 0, 1.2, 0),
    ]
}

/// Publish the list, let the policy take it, then raise the dialog.
pub(super) fn seed_realm_list(
    ctx: Res<CaptureCtx>,
    mut step: Local<u8>,
    mut lists: MessageWriter<crate::net::RealmListMessage>,
    mut realms: ResMut<crate::realm_select::Realms>,
    choice: Res<crate::net::RealmChoice>,
) {
    if ctx.name != NAME {
        return;
    }
    match *step {
        0 => {
            lists.write(crate::net::RealmListMessage { realms: fixture() });
        }
        2 => crate::realm_select::open_over_char_select(&mut realms, &choice),
        3.. => return,
        _ => {}
    }
    *step += 1;
}
