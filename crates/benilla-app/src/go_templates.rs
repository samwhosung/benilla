//! The ask-once GameObject template cache, filled by `CMSG_GAMEOBJECT_QUERY` when an object
//! streams in. The template's lockId decides a right-click: a locked object is opened by an
//! `OPEN_LOCK` cast, an unlocked one by `CMSG_GAMEOBJ_USE`.

use benilla_formats::{LockCatalog, LockTypeCatalog};
use benilla_protocol::guid;
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::QueryCache;

/// `Lock.dbc`: decides use against cast, and which `LockType` the opener spell must match.
#[derive(Resource)]
pub(crate) struct Locks(pub(crate) LockCatalog);

/// `LockType.dbc`: names a lockable object's cursor (`PickLock`, `GatherHerbs`, `Mine`) through
/// the client's lock, LockType, CursorName chain.
#[derive(Resource)]
pub(crate) struct LockTypes(pub(crate) LockTypeCatalog);

/// A cached GameObject template, as the interact routing and the hover tooltip need it.
#[derive(Clone)]
pub(crate) struct GoTemplate {
    /// The `Lock.dbc` id; `0` is no lock.
    pub(crate) lock_id: u32,
    /// The hover tooltip's first line.
    pub(crate) name: String,
    /// Mouseover eligibility for the two types that read a column: GENERIC(5) `data[1]`
    /// (`0x5f4830`) and CAPTURE_POINT(29) `data[19]` (`0x5f6d80`).
    pub(crate) highlight_column: bool,
    /// GENERIC(5) `data[0]` (`0x5f8630`, semantic `0x13`, which no other type has): the tooltip
    /// follows the cursor (`0x492a01`, SetOwner `0x52ffe0` at anchor state 6) or takes the default
    /// corner (`0x492a42`).
    /// Placement, not eligibility: a hoverable object can still be corner-seated.
    pub(crate) floating_tooltip: bool,
    /// MEETINGSTONE (type 23) only.
    pub(crate) meeting_stone: Option<MeetingStoneTemplate>,
    /// MO_TRANSPORT (type 15) only: the transport timetable's inputs.
    pub(crate) mo_transport: Option<MoTransport>,
    /// TEXT (type 9) only; a right-click reads it only when `page_id` is nonzero.
    pub(crate) text_page: Option<TextPage>,
    /// The `PageTextMaterial.dbc` id `GetQuestBackgroundMaterial` answers for a quest from this
    /// object (`0x5f5950`): QUESTGIVER (2) and TEXT (9) read `data[2]`, GOOBER (10) `data[9]`.
    pub(crate) quest_material: Option<u32>,
}

/// A MEETINGSTONE template's slots, read through `0x621b00` keys `0x33` to `0x35` (vmangos
/// `GameObjectInfo::meetingstone`). The area feeds the type's highlightable slot `0x5f6990`, the
/// levels the use refusal `ERR_MEETING_STONE_INVALID_LEVEL`.
#[derive(Clone, Copy)]
pub(crate) struct MeetingStoneTemplate {
    /// `data[0]`.
    pub(crate) min_level: u32,
    /// `data[1]`.
    pub(crate) max_level: u32,
    /// `data[2]`, the `AreaTable` id the stone queues for.
    pub(crate) area: u32,
}

/// A MO_TRANSPORT template's path tuple (`gameobject_template.data0..2`).
#[derive(Clone, Copy)]
pub(crate) struct MoTransport {
    pub(crate) taxi_path_id: u32,
    pub(crate) move_speed: f32,
    pub(crate) accel_rate: f32,
}

/// A TEXT template's readable head (vmangos `GameObjectInfo::text`; the client's `0x621b00` key
/// `0x11` is the same material slot).
#[derive(Clone, Copy)]
pub(crate) struct TextPage {
    /// `data[0]`, the first `PageText` id; `0` is nothing to read.
    pub(crate) page_id: u32,
    /// `data[2]`, a `PageTextMaterial.dbc` id.
    pub(crate) material: u32,
}

/// Entry to template, asked once per connection.
#[derive(Resource, Default)]
pub(crate) struct GameObjectTemplates {
    templates: QueryCache<u32, GoTemplate>,
}

impl crate::query_cache::AskOnce for GameObjectTemplates {
    fn clear_pending(&mut self) {
        self.templates.clear_pending();
    }
}

impl GameObjectTemplates {
    /// Asks for a template once per entry; every spawn of a template shares the one query.
    pub(crate) fn request(&self, guid: u64, commands: &NetCommands) {
        let Some(entry) = guid::entry(guid) else {
            return;
        };
        self.templates.get_or_ask(entry, || {
            debug!("go: asking template (entry {entry}, guid {guid:#x})");
            let _ = commands
                .0
                .send(ClientCommand::GameObjectQuery { entry, guid });
        });
    }

    /// Records a `SMSG_GAMEOBJECT_QUERY_RESPONSE`; a miss arrives zeroed, so no lock.
    pub(crate) fn insert(&mut self, entry: u32, type_id: u32, name: String, data: &[i32; 24]) {
        let lock_id = go_lock_slot(type_id)
            .and_then(|slot| data.get(slot))
            .map(|&v| v.max(0) as u32)
            .unwrap_or(0);
        // The reference resolves both slots by `0x621b00(type, semantic 0x12)`.
        let highlight_column = match type_id {
            5 => data[1] != 0,
            29 => data[19] != 0,
            _ => false,
        };
        // Adjacent to the highlight column but a different question: placement.
        let floating_tooltip = type_id == 5 && data[0] != 0;
        let meeting_stone = (type_id == 23).then(|| MeetingStoneTemplate {
            min_level: data[0].max(0) as u32,
            max_level: data[1].max(0) as u32,
            area: data[2].max(0) as u32,
        });
        // vmangos `GameObjectInfo::moTransport`.
        let mo_transport = (type_id == 15).then(|| MoTransport {
            taxi_path_id: data[0].max(0) as u32,
            move_speed: data[1].max(0) as f32,
            accel_rate: data[2].max(0) as f32,
        });
        let text_page = (type_id == 9).then(|| TextPage {
            page_id: data[0].max(0) as u32,
            material: data[2].max(0) as u32,
        });
        let quest_material = match type_id {
            2 | 9 => Some(data[2].max(0) as u32),
            10 => Some(data[9].max(0) as u32),
            _ => None,
        };
        self.templates.insert(
            entry,
            Some(GoTemplate {
                lock_id,
                name,
                highlight_column,
                floating_tooltip,
                meeting_stone,
                mo_transport,
                text_page,
                quest_material,
            }),
        );
    }

    /// The cached template for a GameObject guid.
    pub(crate) fn get(&self, guid: u64) -> Option<&GoTemplate> {
        guid::entry(guid).and_then(|e| self.templates.get(e))
    }
}

/// The `data[]` slot of a type's lockId (vmangos `GameObjectInfo::GetLockId`).
fn go_lock_slot(type_id: u32) -> Option<usize> {
    match type_id {
        // DOOR(0), BUTTON(1): data[0] = startOpen, data[1] = lockId.
        0 | 1 => Some(1),
        // QUESTGIVER(2), CHEST(3, also gathering nodes), TRAP(6), GOOBER(10), AREADAMAGE(12),
        // CAMERA(13), FLAGSTAND(24), FLAGDROP(26).
        2 | 3 | 6 | 10 | 12 | 13 | 24 | 26 => Some(0),
        // FISHINGHOLE(25).
        25 => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_slot_by_type_matches_getlockid() {
        assert_eq!(go_lock_slot(3), Some(0)); // CHEST (and gathering nodes)
        assert_eq!(go_lock_slot(0), Some(1)); // DOOR (slot 0 is startOpen)
        assert_eq!(go_lock_slot(1), Some(1)); // BUTTON
        assert_eq!(go_lock_slot(25), Some(4)); // FISHINGHOLE
        assert_eq!(go_lock_slot(5), None); // GENERIC
        assert_eq!(go_lock_slot(19), None); // MAILBOX opens by USE
    }

    /// `0x5f5950`: QUESTGIVER (2) and TEXT (9) read `data[2]`, GOOBER (10) `data[9]`.
    #[test]
    fn insert_captures_the_quest_material_by_type() {
        let mut t = GameObjectTemplates::default();
        let mut data = [0i32; 24];
        data[2] = 2;
        data[9] = 3;
        t.insert(100, 2, "Wanted Poster".into(), &data);
        t.insert(101, 9, "Book".into(), &data);
        t.insert(102, 10, "Chest".into(), &data);
        t.insert(103, 15, "Boat".into(), &data);
        assert_eq!(t.templates.get(100).unwrap().quest_material, Some(2));
        assert_eq!(t.templates.get(101).unwrap().quest_material, Some(2));
        assert_eq!(
            t.templates.get(102).unwrap().quest_material,
            Some(3),
            "a chest reads data[9]"
        );
        assert_eq!(t.templates.get(103).unwrap().quest_material, None);
    }

    #[test]
    fn insert_captures_mo_transport_tuple() {
        let mut t = GameObjectTemplates::default();
        let mut data = [0i32; 24];
        // The Menethil to Theramore boat's vmangos template row.
        data[0] = 292;
        data[1] = 30;
        data[2] = 1;
        t.insert(176231, 15, "Proudmore's Treasure".into(), &data);
        let mo = t
            .templates
            .get(176231)
            .unwrap()
            .mo_transport
            .expect("type 15 captures");
        assert_eq!(mo.taxi_path_id, 292);
        assert_eq!(mo.move_speed, 30.0);
        assert_eq!(mo.accel_rate, 1.0);
        // A chest doesn't.
        t.insert(2, 3, "Chest".into(), &data);
        assert!(t.templates.get(2).unwrap().mo_transport.is_none());
    }

    #[test]
    fn insert_captures_the_text_page_head() {
        let mut t = GameObjectTemplates::default();
        let mut data = [0i32; 24];
        data[0] = 1416; // pageID
        data[1] = 0; // language
        data[2] = 2; // pageMaterial (Stone)
        t.insert(2036, 9, "Book".into(), &data);
        let page = t
            .templates
            .get(2036)
            .unwrap()
            .text_page
            .expect("type 9 captures");
        assert_eq!(page.page_id, 1416);
        assert_eq!(page.material, 2);
        // A goober's data[0] is a lockId.
        t.insert(2037, 10, "Lever".into(), &data);
        assert!(t.templates.get(2037).unwrap().text_page.is_none());
    }

    #[test]
    fn insert_resolves_chest_lockid_from_slot_0() {
        let mut t = GameObjectTemplates::default();
        let mut data = [0i32; 24];
        data[0] = 38; // a Copper Vein's lockId lives in chest slot 0
                      // HIGHGUID_GAMEOBJECT with entry 1731 in bits 24..47.
        let guid = 0xF110_0000_0000_0000 | (1731u64 << 24) | 0x40;
        t.insert(
            guid::entry(guid).unwrap(),
            3,
            "Alliance Chest".into(),
            &data,
        );
        assert_eq!(t.get(guid).map(|g| g.lock_id), Some(38));
    }

    /// Real rows from the 1.12 client's `gameobjectcache.wdb`: two pointer signs, both hoverable,
    /// one cursor-seated and one corner-seated.
    #[test]
    fn a_generic_can_be_hoverable_and_still_corner_seated() {
        let mut t = GameObjectTemplates::default();
        let mut brill = [0i32; 24];
        (brill[0], brill[1]) = (1, 1);
        let mut pointer = [0i32; 24];
        (pointer[0], pointer[1]) = (0, 1);
        t.insert(1630, 5, "Brill".into(), &brill);
        t.insert(175656, 5, "Doodad_WoodSignPointerNice10".into(), &pointer);

        let brill = t.templates.get(1630).expect("Brill cached");
        assert!(brill.highlight_column, "Brill is hoverable");
        assert!(brill.floating_tooltip, "Brill's plate follows the cursor");

        let pointer = t.templates.get(175656).expect("the pointer sign cached");
        assert!(
            pointer.highlight_column,
            "the pointer sign is hoverable too"
        );
        assert!(
            !pointer.floating_tooltip,
            "but its plate takes the corner seat — eligibility did not decide placement"
        );
    }

    /// `0x621b00` answers semantic `0x13` for GENERIC(5) alone.
    #[test]
    fn no_other_type_reads_data0_as_the_placement_fork() {
        let mut t = GameObjectTemplates::default();
        let mut data = [0i32; 24];
        data[0] = 1;
        for type_id in [0u32, 1, 2, 3, 6, 7, 8, 9, 10, 16, 22, 23, 25, 29] {
            t.insert(9000 + type_id, type_id, format!("type {type_id}"), &data);
            assert!(
                !t.templates
                    .get(9000 + type_id)
                    .expect("cached")
                    .floating_tooltip,
                "type {type_id} must not take the cursor arm — it carries no semantic 0x13"
            );
        }
    }
}
