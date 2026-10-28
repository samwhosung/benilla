//! The mail getters' reads at the call, the call host's half of [`benilla_ui::script::MailBoxQuery`],
//! [`benilla_ui::script::MailRowQuery`], [`benilla_ui::script::NewMailQuery`],
//! [`benilla_ui::script::StationeryQuery`], [`benilla_ui::script::StationeryTextureQuery`] and
//! [`benilla_ui::script::PackageQuery`]: the mailbox cell `[0xb6ef88]` and the records as the
//! session holds them, each row resolved off the caches as the reference's getters read them as
//! they run (`0x4aebc0`, `0x4af110`, `0x4af360`, `0x4af5d0`), the pending-mail time `0x845eac`
//! (`0x4afea0`), the usable stationery list as last rebuilt (`0x4ad970`), the Stationery store's
//! textures by id (`0x4ae3f0`) and `Package.dbc`'s rows.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    MailBox, MailBoxQuery, MailInboxRow, MailRowQuery, NewMailQuery, PackageQuery, StationeryQuery,
    StationeryTextureQuery, StationeryView,
};

use super::{
    package_view, resolve_row, MailOpen, MailPending, MailTemplates, Packages, RowCatalogs,
    Stationery,
};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::query_cache::Ask;

/// What the mail getters read as they run; the session absent in a harness without it, where they
/// read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct MailLive<'w, 's> {
    mail: Option<Res<'w, MailOpen>>,
    pending: Option<Res<'w, MailPending>>,
    items: Option<Res<'w, Items>>,
    icons: Option<Res<'w, ItemDisplays>>,
    names: Option<Res<'w, NameCache>>,
    stationery: Option<Res<'w, Stationery>>,
    templates: Option<Res<'w, MailTemplates>>,
    packages: Option<Res<'w, Packages>>,
    houses: Option<Res<'w, crate::ui_auction::AuctionHouses>>,
    gameobjects: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    states: Option<Res<'w, crate::world_state::WorldStates>>,
    rolls: crate::items::Rolls<'w>,
    unit_names: crate::ui_unit::NameTables<'w>,
    /// The local player, the letter body's `$`-macro subject.
    me: Query<
        'w,
        's,
        (&'static crate::net::ObjectStore, &'static crate::net::Guid),
        With<crate::net::SelfPlayer>,
    >,
    /// The usable stationery list as last rebuilt, and its rebuild's reads.
    paper: super::StationeryRebuild<'w, 's>,
}

impl MailLive<'_, '_> {
    /// The cell and the count; `None` with no session to read.
    fn mailbox(&self) -> Option<MailBox> {
        let mail = self.mail.as_deref()?;
        Some(MailBox {
            open: mail.mailbox.is_some(),
            count: mail.mails.len(),
        })
    }

    /// The 0-based row `index`, its caches only read; `None` with no session to read.
    fn row(&self, q: &MailRowQuery) -> Option<Option<MailInboxRow>> {
        let (mail, items, names) = (
            self.mail.as_deref()?,
            self.items.as_deref()?,
            self.names.as_deref()?,
        );
        let Some(entry) = mail.mails.get(q.index) else {
            return Some(None);
        };
        let default_states = crate::world_state::WorldStates::default();
        let states = self.states.as_deref().unwrap_or(&default_states);
        // `None` until the player is named.
        let subject = crate::npc_text::player_identity(&self.me, names, self.unit_names.get());
        let macros = crate::npc_text::MacroContext {
            subject: subject.as_ref(),
            states,
        };
        let get_text = |key: &str| {
            q.strings
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        };
        Some(Some(resolve_row(
            entry,
            &mail.bodies,
            mail.auction_subjects.get(&entry.message_id),
            items,
            self.icons.as_deref(),
            self.stationery.as_deref(),
            self.templates.as_deref(),
            RowCatalogs {
                packages: self.packages.as_deref(),
                houses: self.houses.as_deref(),
                gameobjects: self.gameobjects.as_deref(),
            },
            names,
            Ask::Read,
            self.rolls.get(),
            &macros,
            &get_text,
        )))
    }

    /// The usable stationery list as last rebuilt, rebuilt first for `GetNumStationeries` while no
    /// paper's item is awaited (`0x4ae1f6`-`0x4ae202`), its reads the player's items as of the
    /// drain's place; `None` with no store to build it from.
    fn stationeries(&mut self, q: &StationeryQuery) -> Option<Vec<StationeryView>> {
        if q.rebuild {
            self.paper.rebuild_unless_awaited(self.items.as_deref()?);
        }
        Some(self.paper.list()?.to_vec())
    }
}

impl crate::script_calls::live::LiveFamily for MailLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &MailBoxQuery| self.mailbox())
            || ask.answer(|q: &MailRowQuery| self.row(q))
            || ask.answer(|_: &NewMailQuery| Some(self.pending.as_deref()?.has_new_mail()))
            || ask.answer(|q: &StationeryQuery| self.stationeries(q))
            || ask.answer(|q: &StationeryTextureQuery| {
                Some(
                    self.stationery
                        .as_deref()?
                        .0
                        .texture_of(q.id)
                        .map(str::to_owned),
                )
            })
            || ask.answer(|_: &PackageQuery| {
                Some(
                    self.packages
                        .as_deref()?
                        .0
                        .iter()
                        .map(package_view)
                        .collect(),
                )
            })
    }
}
