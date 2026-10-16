//! The realm-list boundary smoke's native seat: the walk ([`crate::glue_session::realm_smoke`])
//! pressing this dialog's own actions.

use bevy::prelude::*;

use crate::client_state::ClientState;
use crate::glue_session::realm_smoke::{walk_frame, Seat, Walk};
use crate::glue_session::roster::Roster;
use crate::net::{CharListMessage, RealmChoice};

use super::Realms;

/// The native dialog.
struct Native<'a> {
    realms: &'a mut Realms,
    choice: &'a RealmChoice,
    generation: u64,
}

impl Seat for Native<'_> {
    fn screen_up(&self) -> bool {
        true
    }
    fn dialog_up(&self) -> bool {
        self.realms.shown
    }
    fn can_open(&self) -> bool {
        !self.realms.realms.is_empty()
    }
    fn realms(&self) -> usize {
        self.realms.realms.len()
    }
    fn open(&mut self) {
        super::open_over_char_select(self.realms, self.choice);
    }
    fn okay(&mut self) {
        let name = self.realms.selected().map(|r| r.name.clone()).or_else(|| {
            self.realms
                .rows()
                .first()
                .map(|&i| self.realms.realms[i].name.clone())
        });
        let Some(name) = name else {
            return;
        };
        self.realms.hide();
        self.realms.enter(self.choice, name, self.generation);
    }
    fn cancel(&mut self) {
        self.realms.hide();
    }
}

/// The walk on the native dialog.
#[allow(clippy::too_many_arguments)]
pub(super) fn debug_realm_smoke(
    mut msgs: MessageReader<CharListMessage>,
    mut realms: ResMut<Realms>,
    roster: Res<Roster>,
    state: Res<State<ClientState>>,
    choice: Res<RealmChoice>,
    abandon: Option<Res<crate::net::LoginAbandon>>,
    time: Res<Time>,
    mut walk: Local<Walk>,
    mut exit: MessageWriter<AppExit>,
) {
    let seen = msgs.read().count() as u32;
    let mut seat = Native {
        realms: &mut realms,
        choice: &choice,
        generation: crate::net::LoginAbandon::now(abandon.as_deref()),
    };
    walk_frame(
        &mut walk,
        seen,
        &roster,
        *state.get(),
        time.elapsed_secs(),
        &mut seat,
        &mut exit,
    );
}
