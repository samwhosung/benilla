//! A tap on the field-notice, item and cast-report lanes, for a test of what a handler queues and in what
//! order. In the world with the interface up the dispatch fires each packet's lanes at its end
//! ([`crate::lanes::flush`]), so the queues a test used to read after the dispatch are empty; the
//! tap takes each packet's runs at that end, ahead of the flush, as the reference's handler
//! raises them.

use bevy::prelude::*;

use crate::field_watch::{FieldNotice, NoticeLane};
use crate::item_lane::ItemLane;
use crate::item_watch::{ItemNotice, Run};
use crate::lanes::Stamp;
use crate::ui_cast::{CastReport, CastReports};

/// What the lanes held at each packet's end, in the order the packets ended, with their stamps.
#[derive(Resource, Default)]
pub(crate) struct Tapped {
    fields: Vec<(Stamp, FieldNotice)>,
    items: Vec<(Stamp, Run)>,
    casts: Vec<CastReport>,
}

impl Tapped {
    /// The field notices tapped so far, taken.
    pub(crate) fn fields(&mut self) -> Vec<FieldNotice> {
        self.fields.drain(..).map(|(_, n)| n).collect()
    }

    /// The field notices tapped so far with their stamps, taken.
    pub(crate) fn fields_stamped(&mut self) -> Vec<(Stamp, FieldNotice)> {
        std::mem::take(&mut self.fields)
    }

    /// The item notices tapped so far, taken.
    pub(crate) fn item_notices(&mut self) -> Vec<ItemNotice> {
        self.items.drain(..).filter_map(|(_, r)| r.notice).collect()
    }

    /// The cast reports tapped so far, taken.
    pub(crate) fn casts(&mut self) -> Vec<CastReport> {
        std::mem::take(&mut self.casts)
    }

    /// The item runs tapped so far with their stamps, taken.
    pub(crate) fn items_stamped(&mut self) -> Vec<(Stamp, Run)> {
        std::mem::take(&mut self.items)
    }
}

/// Tap `app`'s dispatch: each packet's field notices and item runs move into [`Tapped`] at its
/// end, unfired.
pub(crate) fn tap(app: &mut App) {
    app.init_resource::<Tapped>();
    crate::net::handlers::add_packet_end(app, take);
}

fn take(world: &mut World, _at: Stamp) {
    let fields: Vec<_> = world.resource_mut::<NoticeLane>().drain_stamped().collect();
    let items = world.resource_mut::<ItemLane>().drain_stamped();
    let casts = world.resource_mut::<CastReports>().take();
    let mut tapped = world.resource_mut::<Tapped>();
    tapped.fields.extend(fields);
    tapped.items.extend(items);
    tapped.casts.extend(casts);
}
