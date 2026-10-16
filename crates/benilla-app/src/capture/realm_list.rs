//! `glue-realmlist`: the stock realm list over the login screen with realms in two categories,
//! which the live server cannot show (it lists one). A session with no characters that has chosen
//! no realm gets the realm wizard instead (`GET_PREFERRED_REALM_INFO`, `0x46e718`), so the run
//! takes the wizard's way to a choice: the list asked for (`RequestRealmList(1)`) and answered as
//! the realm park answers, the wizard's first language suggested (`SetPreferredInfo`), the
//! suggestion dismissed back to the login screen, and the list asked for and answered again, which
//! the glue manager now raises as the list. The tabs are named from the install's own Region, so
//! bytes 1 and 2 are "English" and "German" under Region 3 and 1 alone under Region 1.

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

/// Walk the login screen to the list: ask, answer, choose through the wizard, ask and answer again.
pub(super) fn seed_realm_list(
    ctx: Res<CaptureCtx>,
    mut step: Local<u8>,
    mut lists: MessageWriter<crate::net::RealmListMessage>,
    stock: crate::ui_script::glue::StockGlueProbe,
) {
    if ctx.name != NAME {
        return;
    }
    let Some(vm) = Some(stock.vm()).filter(|vm| vm.is_up()) else {
        return;
    };
    let answer = |lists: &mut MessageWriter<crate::net::RealmListMessage>| {
        lists.write(crate::net::RealmListMessage {
            realms: fixture(),
            failed: false,
        });
    };
    match *step {
        0 if super::glue_press::login_form_up(vm) => vm.press("RequestRealmList(1)"),
        1 | 4 => answer(&mut lists),
        2 if vm.frame_visible("RealmWizard") => {
            vm.press("RealmWizardLocationButton_OnClick(1) SetPreferredInfo(1, nil, nil)")
        }
        3 if vm.frame_visible("GlueDialog") => {
            vm.press("GlueDialog:Hide() SetGlueScreen(\"login\") RequestRealmList(1)")
        }
        _ => return,
    }
    *step += 1;
}
