//! An unattended run's reconnect on the stock screens: the session lost while a pick is kept for it
//! ([`crate::net::DisconnectedMessage`]'s `session_over` false), the login resubmitted
//! ([`super::login`]), the run answers the realm list with the realm it was on and asks the
//! roster, which the roster policy answers with the kept pick ([`super::roster`]).
//!
//! No glue screen stands to make these asks: the character is still in the world, the world's VM
//! up and the glue torn down, so the stock screens' connect and their `GetCharacterListUpdate`
//! never run. The reference has no reconnect at all (a lost session is `DISCONNECTED_FROM_SERVER`).

use bevy::prelude::*;

use super::roster::Roster;
use crate::net::{
    CharPick, CharRequest, LoginAbandon, RealmChoice, RealmListMessage, RealmRequest,
    WorldAuthMessage,
};

/// Answer the realm list and the character park of a reconnect while a pick is kept: the realm the
/// session was on, as the list now holds it, then the roster ask.
pub(super) fn answer_reconnect(
    roster: Res<Roster>,
    mut lists: MessageReader<RealmListMessage>,
    mut auths: MessageReader<WorldAuthMessage>,
    choice: Res<RealmChoice>,
    pick: Res<CharPick>,
    abandon: Option<Res<LoginAbandon>>,
) {
    let kept = roster.pending_pick.is_some();
    for list in lists.read() {
        if !kept || list.failed {
            continue;
        }
        let Some(name) = roster.realm.as_ref().map(|r| r.name.as_str()) else {
            continue;
        };
        match list.realms.iter().find(|r| r.name == name) {
            Some(realm) => {
                info!("reconnect: dialing {name} again for the kept pick");
                let _ = choice.0.send(RealmRequest::Enter {
                    realm: realm.clone(),
                    generation: LoginAbandon::now(abandon.as_deref()),
                });
            }
            None => warn!("reconnect: {name} is not on the realm list — the pick is not re-made"),
        }
    }
    if auths.read().last().is_some() && kept {
        let _ = pick.0.send(CharRequest::Enumerate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::glue_session::roster::test_character;

    fn realm(name: &str) -> benilla_protocol::RealmInfo {
        benilla_protocol::RealmInfo {
            name: name.into(),
            address: "127.0.0.1:8085".into(),
            population: 1.0,
            characters: 1,
            realm_type: 0,
            flags: 0,
            category: 1,
            id: 0,
        }
    }

    type Rx<T> = crossbeam_channel::Receiver<T>;

    fn reconnect_app(pending: bool) -> (App, Rx<RealmRequest>, Rx<CharRequest>) {
        let (realm_tx, realm_rx) = crossbeam_channel::unbounded();
        let (pick_tx, pick_rx) = crossbeam_channel::unbounded();
        let mut roster = if pending {
            Roster::with_pending_pick(vec![test_character(7, "Seven")], 7)
        } else {
            Roster::with_chars(vec![test_character(7, "Seven")])
        };
        roster.realm = Some(realm("Beta"));
        let mut app = App::new();
        app.insert_resource(roster)
            .insert_resource(RealmChoice(realm_tx.into()))
            .insert_resource(CharPick(pick_tx.into()))
            .add_message::<RealmListMessage>()
            .add_message::<WorldAuthMessage>()
            .add_systems(Update, answer_reconnect);
        (app, realm_rx, pick_rx)
    }

    fn reconnect(app: &mut App) {
        app.world_mut().write_message(RealmListMessage {
            realms: vec![realm("Alpha"), realm("Beta")],
            failed: false,
        });
        app.update();
        app.world_mut().write_message(WorldAuthMessage {
            realm: realm("Beta"),
            epoch: 2,
            generation: 0,
        });
        app.update();
    }

    /// With a pick kept, the reconnect dials the realm the session was on and asks the roster
    /// the pick is re-made from; with none, a realm list and an authentication ask nothing, as at
    /// a first login, where the stock screens make both asks.
    #[test]
    fn a_kept_pick_dials_its_realm_and_asks_the_roster() {
        let (mut app, realms, picks) = reconnect_app(true);
        reconnect(&mut app);
        assert!(matches!(
            realms.try_recv(),
            Ok(RealmRequest::Enter { realm, .. }) if realm.name == "Beta"
        ));
        assert!(matches!(picks.try_recv(), Ok(CharRequest::Enumerate)));

        let (mut app, realms, picks) = reconnect_app(false);
        reconnect(&mut app);
        assert!(realms.try_recv().is_err());
        assert!(picks.try_recv().is_err());
    }
}
