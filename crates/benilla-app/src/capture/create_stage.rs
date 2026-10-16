//! `glue-charcreate`: the stock create screen with one body up, its race's stage behind it. Once
//! the login screen stands the run presses Create New Character's `SetGlueScreen("charcreate")`,
//! then the body's race, sex and class buttons, every customization at its first choice, so the
//! shot does not hang on the clock-seeded draws `ResetCharCustomize` makes
//! ([`super::glue_press::press_body`]).

use bevy::prelude::*;

use benilla_protocol::CharCreateReq;

use super::glue_press;
use crate::ui_script::glue::StockGlueProbe;

/// The body the create capture puts up: `(race, sex, class)` ids, the scenario's own or
/// `WOW_CHARCREATE_PICK="race,sex,class"`'s (`6,0,11` is the tauren stage).
#[derive(Resource, Clone, Copy)]
pub(super) struct CreatePick(pub(super) (u8, u8, u8));

impl CreatePick {
    /// The scenario's pick, unless the environment names another.
    pub(super) fn resolve(scenario: (u8, u8, u8)) -> Self {
        let env = std::env::var("WOW_CHARCREATE_PICK").ok().and_then(|v| {
            let ids: Vec<u8> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            (ids.len() == 3).then(|| (ids[0], ids[1], ids[2]))
        });
        CreatePick(env.unwrap_or(scenario))
    }
}

/// Put the create screen up over the login screen, then the pick's body on it.
pub(super) fn seed_create_stage(
    pick: Option<Res<CreatePick>>,
    stock: StockGlueProbe,
    sources: crate::ui_script::glue_create::CreateSources,
    mut step: Local<u8>,
) {
    let Some(CreatePick((race, sex, class))) = pick.as_deref().copied() else {
        return;
    };
    let Some(vm) = Some(stock.vm()).filter(|vm| vm.is_up()) else {
        return;
    };
    match *step {
        // A frame after the last catalog arrives, once the glue VM is seated with it.
        0 if glue_press::login_form_up(vm) && sources.complete() && !sources.changed() => {
            glue_press::press_create_screen(vm);
            *step = 1;
        }
        1 if glue_press::create_up(vm) => {
            let req = CharCreateReq {
                name: String::new(),
                race,
                class,
                gender: sex,
                skin: 0,
                face: 0,
                hair_style: 0,
                hair_color: 0,
                facial_hair: 0,
            };
            if let Err(why) = glue_press::press_body(vm, &req) {
                error!("capture: the create screen refused the pick ({why})");
            }
            *step = 2;
        }
        _ => {}
    }
}
