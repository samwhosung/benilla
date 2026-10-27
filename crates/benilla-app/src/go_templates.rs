//! The ask-once GameObject template cache, filled by `CMSG_GAMEOBJECT_QUERY` when an object
//! streams in. The template's lockId decides a right-click: a locked object is opened by an
//! `OPEN_LOCK` cast, an unlocked one by `CMSG_GAMEOBJ_USE`.

use benilla_assets::LockRecover;
use benilla_formats::{LockCatalog, LockTypeCatalog};
use benilla_protocol::messages::{ObjectFields, ObjectType};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands, Objects};
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
    /// The 24 data dwords (`[rec+0x1c]`), read by semantic through [`GoTemplate::column`].
    data: [i32; 24],
}

impl GoTemplate {
    /// `0x5f8150(0x621b00(type, id))` for the object `go` of this template: the data column its
    /// own `GAMEOBJECT_TYPE_ID` names `id` by (`[go+0x110]+0x3c`, as `0x5f5900`, `0x5f5970` and
    /// `0x5f59a0` read it), 0 for a type without one (`0x5f8150` refuses the −1 as past 23).
    pub(crate) fn column(&self, go: &ObjectFields, id: GoColumn) -> i32 {
        u32::try_from(go.gameobject_type_id())
            .ok()
            .and_then(|t| data_column(t, id))
            .map_or(0, |c| self.data[c])
    }

    /// The first `PageText` id the reader opens on, the pageID column (`GetPageTextId 0x5f5900`):
    /// TEXT's `data[0]`, GOOBER's `data[7]`; 0 is nothing to read.
    pub(crate) fn page_id(&self, go: &ObjectFields) -> u32 {
        self.column(go, GoColumn::PageId).max(0) as u32
    }

    /// The `Languages.dbc` id its page text is written in, the language column (`0x4e37fc` →
    /// `0x5f5930`, `0x621b00` id `0x10`): TEXT's `data[1]`, GOOBER's `data[8]`; 0 for a type
    /// without one.
    pub(crate) fn language(&self, go: &ObjectFields) -> u32 {
        self.column(go, GoColumn::Language).max(0) as u32
    }

    /// The `PageTextMaterial.dbc` id, the pageMaterial column (`0x5f5950`), which the reader and
    /// `GetQuestBackgroundMaterial` paint on: QUESTGIVER's and TEXT's `data[2]`, GOOBER's
    /// `data[9]`; 0 for a type without one.
    pub(crate) fn page_material(&self, go: &ObjectFields) -> u32 {
        self.column(go, GoColumn::PageMaterial).max(0) as u32
    }
}

/// The data-column semantic ids benilla reads, by their names in the record table at `0x80c9fc`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GoColumn {
    /// `startOpen` (1).
    StartOpen,
    /// `autoClose` (3).
    AutoClose,
    /// `pageID` (0xf).
    PageId,
    /// `language` (0x10).
    Language,
    /// `pageMaterial` (0x11).
    PageMaterial,
    /// `noDamageImmune` (0x39).
    NoDamageImmune,
    /// `notInCombat` (0x3e).
    NotInCombat,
}

/// Whether the object's own type has an `id` column at all (`0x621b00` gives no −1).
pub(crate) fn has_column(go: &ObjectFields, id: GoColumn) -> bool {
    u32::try_from(go.gameobject_type_id())
        .ok()
        .and_then(|t| data_column(t, id))
        .is_some()
}

/// `0x621b00(type, id)` for the ids of [`GoColumn`]: the index of `id` in the type's id array
/// (`[0x80d7a4 + 20·type]`, counted by `[0x80d7a0 + 20·type]`), or `None` where the type has no
/// such column.
fn data_column(type_id: u32, id: GoColumn) -> Option<usize> {
    use GoColumn::*;
    match (type_id, id) {
        (0 | 1, StartOpen) => Some(0),
        (0 | 1, AutoClose) => Some(2),
        (0, NoDamageImmune) => Some(3),
        (1, NoDamageImmune) => Some(4),
        (2, PageMaterial) => Some(2),
        (2, NoDamageImmune) => Some(5),
        (3, NotInCombat) => Some(12),
        (6, AutoClose) => Some(6),
        (9, PageId) => Some(0),
        (9, Language) => Some(1),
        (9, PageMaterial) => Some(2),
        (10, AutoClose) => Some(3),
        (10, PageId) => Some(7),
        (10, Language) => Some(8),
        (10, PageMaterial) => Some(9),
        (10, NoDamageImmune) => Some(11),
        (12, AutoClose) => Some(5),
        (24, NoDamageImmune) => Some(5),
        (26, NoDamageImmune) => Some(3),
        (30, StartOpen) => Some(0),
        _ => None,
    }
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
    /// `data[1]` and `data[2]` as the dwords the client makes floats (`0x5f4cc0`).
    pub(crate) move_speed: u32,
    pub(crate) accel_rate: u32,
}

/// Entry to template, asked once while in flight.
#[derive(Resource, Default)]
pub(crate) struct GameObjectTemplates {
    templates: QueryCache<u32, GoTemplate>,
    /// The entries the quest log's leaderboard missed, each waiting with the callback `0x4e0800`
    /// (`0x4e0387`). Deviation: one per entry, where the reference keeps one per call, as the
    /// creature arm's waits are kept (`benilla_ui::script::quest_log`).
    quest_waits: std::sync::Mutex<Vec<u32>>,
}

impl GameObjectTemplates {
    /// The key an object's template is cached under: its descriptor's `OBJECT_FIELD_ENTRY`
    /// (`[descriptor+0xc]`, `0x5f7d77`), never the guid, whose bits carry the entry only on some
    /// cores. `None` for a descriptor that has not received the field.
    fn key(fields: &ObjectFields) -> Option<u32> {
        fields.object_entry().filter(|&e| e != 0)
    }

    /// The creation hook's ask: it looks the template up with a callback, by the descriptor's
    /// entry and the guid (`0x5f7c40`, `0x5f7d81`), so a refused entry is asked again by the next
    /// spawn. Every spawn of a template shares the one pending query.
    pub(crate) fn request_at_create(
        &self,
        guid: u64,
        fields: &ObjectFields,
        commands: &NetCommands,
    ) {
        if let Some(entry) = Self::key(fields) {
            self.templates
                .lookup(entry, || Self::send(entry, guid, commands));
        }
    }

    fn send(entry: u32, guid: u64, commands: &NetCommands) {
        debug!("go: asking template (entry {entry}, guid {guid:#x})");
        let _ = commands
            .0
            .send(ClientCommand::GameObjectQuery { entry, guid });
    }

    /// The template store, the one [`crate::wdb`] saves.
    pub(crate) fn template_store(&self) -> &QueryCache<u32, GoTemplate> {
        &self.templates
    }

    pub(crate) fn template_store_mut(&mut self) -> &mut QueryCache<u32, GoTemplate> {
        &mut self.templates
    }

    /// A refused entry (`SMSG_GAMEOBJECT_QUERY_RESPONSE` with the top bit set, `0x5588e0`): no
    /// template, and the next lookup asks again ([`QueryCache::refuse`]).
    pub(crate) fn refuse(&mut self, entry: u32) {
        self.templates.refuse(entry);
    }

    /// Whether the server refused `entry` and nothing has asked for it since.
    #[cfg(test)]
    pub(crate) fn is_unknown(&self, entry: u32) -> bool {
        self.templates.answered_unknown(entry)
    }

    /// Records a `SMSG_GAMEOBJECT_QUERY_RESPONSE`.
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
            move_speed: data[1] as u32,
            accel_rate: data[2] as u32,
        });
        self.templates.insert(
            entry,
            GoTemplate {
                lock_id,
                name,
                highlight_column,
                floating_tooltip,
                meeting_stone,
                mo_transport,
                data: *data,
            },
        );
    }

    /// The leaderboard's lookup of `entry` inside its call: a miss asks, a refused entry again, and
    /// waits with the callback `0x4e0800` (`0x4e0387`-`0x4e039c`).
    pub(crate) fn ask_for_quest(&self, entry: u32, commands: &NetCommands) {
        if self
            .resolve_name(entry, crate::query_cache::Ask::Event(commands))
            .is_none()
        {
            let mut waits = self.quest_waits.lock_recover();
            if !waits.contains(&entry) {
                waits.push(entry);
            }
        }
    }

    /// The answer for `entry` landed, or was refused: whether the leaderboard waited on it, the
    /// wait spent either way.
    pub(crate) fn take_quest_wait(&mut self, entry: u32) -> bool {
        let waits = self
            .quest_waits
            .get_mut()
            .unwrap_or_else(|e| e.into_inner());
        let before = waits.len();
        waits.retain(|&e| e != entry);
        waits.len() != before
    }

    /// A template's name by entry, read without a callback (the cache `0xc0e318`).
    pub(crate) fn name(&self, entry: u32) -> Option<&str> {
        self.templates.get(entry).map(|t| t.name.as_str())
    }

    /// A template's name by entry, read as `ask` says, a miss asked with no spawn's guid: the inbox
    /// header's game object sender (`0x558560` with the callback `0x4af080`, `0x4aee94`).
    pub(crate) fn resolve_name(
        &self,
        entry: u32,
        ask: crate::query_cache::Ask<'_>,
    ) -> Option<&str> {
        self.templates
            .read(entry, ask, |commands| Self::send(entry, 0, commands))
            .map(|t| t.name.as_str())
    }

    /// The cached template for a GameObject, found through its own descriptor.
    pub(crate) fn of(&self, fields: &ObjectFields) -> Option<&GoTemplate> {
        Self::key(fields).and_then(|e| self.templates.get(e))
    }

    /// [`Self::of`] for a guid, through the streamed object (`ClntObjMgrObjectPtr`, `0x468460`);
    /// `None` for an object that is not streamed, and for any kind but a GameObject, whose entry
    /// names another table.
    pub(crate) fn of_guid(&self, objects: &Objects, guid: u64) -> Option<&GoTemplate> {
        objects
            .object(guid)
            .filter(|f| f.object_type() == Some(ObjectType::GameObject))
            .and_then(|f| self.of(f))
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

/// A GameObject's descriptor as a create leaves it: `OBJECT_FIELD_TYPE` and `OBJECT_FIELD_ENTRY`.
#[cfg(test)]
pub(crate) fn test_fields(entry: u32) -> ObjectFields {
    use benilla_protocol::field::FIELD_OBJECT_ENTRY;
    // `OBJECT_FIELD_TYPE` is field 2: `TYPEMASK_OBJECT | TYPEMASK_GAMEOBJECT`.
    ObjectFields::from_pairs(&[(2, 0x21), (FIELD_OBJECT_ENTRY, entry)])
}

/// [`test_fields`] with `GAMEOBJECT_TYPE_ID` (field 21), which names the object's data columns.
#[cfg(test)]
pub(crate) fn test_typed_fields(entry: u32, type_id: u32) -> ObjectFields {
    use benilla_protocol::field::FIELD_OBJECT_ENTRY;
    ObjectFields::from_pairs(&[(2, 0x21), (FIELD_OBJECT_ENTRY, entry), (21, type_id)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GameObject's descriptor of `GAMEOBJECT_TYPE_ID` (field 21) `type_id`.
    fn go(type_id: u32) -> ObjectFields {
        ObjectFields::from_pairs(&[(2, 0x21), (21, type_id)])
    }

    /// `0x621b00`'s arrays name the door's columns `startOpen`, `open`, `autoClose`,
    /// `noDamageImmune` (0 to 3), the chest's `notInCombat` 12 and the goober's `noDamageImmune`
    /// 11; a type without the id reads 0.
    #[test]
    fn columns_are_read_by_the_types_semantic_ids() {
        let mut t = GameObjectTemplates::default();
        let data: [i32; 24] = std::array::from_fn(|i| 100 + i as i32);
        t.insert(1, 0, "Door".into(), &data);
        t.insert(2, 3, "Chest".into(), &data);
        t.insert(3, 10, "Goober".into(), &data);
        let col = |entry, id| {
            let ty = [0, 0, 3, 10][entry as usize];
            t.templates.get(entry).unwrap().column(&go(ty), id)
        };
        assert_eq!(col(1, GoColumn::StartOpen), 100);
        assert_eq!(col(1, GoColumn::AutoClose), 102);
        assert_eq!(col(1, GoColumn::NoDamageImmune), 103);
        assert_eq!(col(2, GoColumn::NotInCombat), 112);
        assert_eq!(
            col(2, GoColumn::NoDamageImmune),
            0,
            "a chest has no such column"
        );
        assert_eq!(col(3, GoColumn::NoDamageImmune), 111);
        assert_eq!(col(3, GoColumn::AutoClose), 103);
    }

    /// TEXT (9) and GOOBER (10) carry a pageID column; a door has none.
    #[test]
    fn the_page_column_is_texts_and_goobers() {
        assert!(has_column(&go(9), GoColumn::PageId));
        assert!(has_column(&go(10), GoColumn::PageId));
        assert!(!has_column(&go(0), GoColumn::PageId));
    }

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
        t.insert(102, 10, "Goober".into(), &data);
        t.insert(103, 15, "Boat".into(), &data);
        assert_eq!(t.templates.get(100).unwrap().page_material(&go(2)), 2);
        assert_eq!(t.templates.get(101).unwrap().page_material(&go(9)), 2);
        assert_eq!(
            t.templates.get(102).unwrap().page_material(&go(10)),
            3,
            "a GOOBER (type 10) reads data[9]"
        );
        assert_eq!(t.templates.get(103).unwrap().page_material(&go(15)), 0);
    }

    /// The language column (`0x621b00` id `0x10`): TEXT (9) reads `data[1]`, GOOBER (10)
    /// `data[8]`, every other type has none.
    #[test]
    fn the_language_column_is_text_data1_and_goober_data8() {
        let mut t = GameObjectTemplates::default();
        let mut data = [0i32; 24];
        data[1] = 7;
        data[8] = 33;
        for (entry, ty) in [(1, 9), (2, 10), (3, 2), (4, 15)] {
            t.insert(entry, ty, "Object".into(), &data);
        }
        let language = |entry, ty| t.templates.get(entry).unwrap().language(&go(ty));
        assert_eq!(language(1, 9), 7);
        assert_eq!(language(2, 10), 33);
        assert_eq!((language(3, 2), language(4, 15)), (0, 0));
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
        assert_eq!(mo.move_speed, 30);
        assert_eq!(mo.accel_rate, 1);
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
        let book = t.templates.get(2036).unwrap();
        assert_eq!(book.page_id(&go(9)), 1416);
        assert_eq!(book.page_material(&go(9)), 2);
        // A goober's data[0] is a lockId; its page is data[7].
        t.insert(2037, 10, "Lever".into(), &data);
        assert_eq!(t.templates.get(2037).unwrap().page_id(&go(10)), 0);
    }

    #[test]
    fn insert_resolves_chest_lockid_from_slot_0() {
        let mut t = GameObjectTemplates::default();
        let mut data = [0i32; 24];
        data[0] = 38; // a Copper Vein's lockId lives in chest slot 0
        t.insert(1731, 3, "Alliance Chest".into(), &data);
        assert_eq!(t.of(&entry_fields(1731)).map(|g| g.lock_id), Some(38));
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

    use super::test_fields as entry_fields;

    /// `0x5f7c40` keys the cache with `[descriptor+0xc]`, the object's `OBJECT_FIELD_ENTRY`
    /// (`0x5f7d77`), and never the guid: a boat whose guid counter is not its template entry is
    /// asked and read by the field.
    #[test]
    fn the_template_is_keyed_by_the_descriptors_entry_not_the_guid_bits() {
        const FIELD_ENTRY: u32 = 176231;
        // A MO_TRANSPORT guid whose low 32 bits are a plain counter.
        let boat = (u64::from(benilla_protocol::guid::HIGH_MO_TRANSPORT) << 48) | 7;
        let fields = entry_fields(FIELD_ENTRY);

        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        let commands = NetCommands(tx);
        let mut t = GameObjectTemplates::default();
        t.request_at_create(boat, &fields, &commands);
        t.request_at_create(boat, &fields, &commands);
        let asked: Vec<_> = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::GameObjectQuery { entry, guid } => (entry, guid),
                _ => panic!("only the template query is sent"),
            })
            .collect();
        assert_eq!(asked, vec![(FIELD_ENTRY, boat)], "asked once, by the field");

        let mut data = [0i32; 24];
        data[0] = 292;
        t.insert(FIELD_ENTRY, 15, "Boat".into(), &data);
        let mo = t.of(&fields).and_then(|g| g.mo_transport);
        assert_eq!(mo.map(|m| m.taxi_path_id), Some(292), "read by the field");

        // A second object of the same template shares the one query.
        t.request_at_create(boat + 2, &fields, &commands);
        assert!(rx.try_recv().is_err(), "the cached entry is not re-asked");
    }

    /// A descriptor that has not received the entry has no template to ask for or read.
    #[test]
    fn a_descriptor_without_an_entry_asks_nothing() {
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        let commands = NetCommands(tx);
        let mut t = GameObjectTemplates::default();
        t.request_at_create(1, &ObjectFields::default(), &commands);
        assert!(rx.try_recv().is_err());
        t.insert(0, 3, "Chest".into(), &[0; 24]);
        assert!(t.of(&ObjectFields::default()).is_none());
    }
}
