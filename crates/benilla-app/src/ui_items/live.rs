//! The item a getter reads as it runs ([`benilla_ui::script::ItemRead`]): the record off the item
//! cache, and an item object's descriptor, each field derived as it is asked for. The bag,
//! inventory and trade snapshots take the same fields from here, so the live read and the frame's
//! push are one derivation.

use benilla_protocol::messages::ItemInfo;
use benilla_protocol::ObjectFields;
use benilla_ui::script::{ItemKey, ItemRead, ItemRoll};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::entities::ItemDisplays;
use crate::items::{Items, RollCatalogs};
use crate::query_cache::Ask;

/// One item as the getters read it: a held object, or a record alone with its slot's roll.
pub(crate) struct LiveItem<'a> {
    entry: u32,
    /// The object, with its guid; `None` for a record read.
    object: Option<(&'a ObjectFields, u64)>,
    /// The record, only read: a getter's own lookup is its call's ask.
    template: Option<&'a ItemInfo>,
    /// A record read's roll; an object's is read off it ([`Self::roll`]).
    roll: ItemRoll,
    items: &'a Items,
    icons: Option<&'a ItemDisplays>,
    rolls: RollCatalogs<'a>,
    /// `ItemClass.dbc` and `ItemSubClass.dbc`, for `GetItemInfo`'s type names.
    classes: ClassNames<'a>,
}

/// `ItemClass.dbc` and `ItemSubClass.dbc`, absent without game data.
#[derive(Clone, Copy, Default)]
pub(crate) struct ClassNames<'a> {
    pub(crate) classes: Option<&'a benilla_formats::ItemClassCatalog>,
    pub(crate) sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
}

impl ClassNames<'_> {
    /// `GetItemInfo`'s `itemType`, the `ItemClass.dbc` name (`0x48e070`).
    pub(crate) fn item_type(&self, t: &ItemInfo) -> Option<String> {
        self.classes?.name(t.class).map(str::to_string)
    }

    /// `GetItemInfo`'s `itemSubType`: `ItemSubClass.dbc`'s verbose name first (`0x48e311`), so a
    /// one-handed sword is "One-Handed Swords" to an addon and "Sword" on the tooltip.
    pub(crate) fn item_sub_type(&self, t: &ItemInfo) -> Option<String> {
        self.sub_classes?
            .name(t.class, t.subclass)
            .map(str::to_string)
    }
}

impl<'a> LiveItem<'a> {
    /// The same item, its record's type names read through `classes`.
    pub(crate) fn with_classes(self, classes: ClassNames<'a>) -> Self {
        LiveItem { classes, ..self }
    }

    /// The item object `fields`, held under `guid`, and its record by its entry.
    pub(crate) fn object(
        fields: &'a ObjectFields,
        guid: u64,
        items: &'a Items,
        icons: Option<&'a ItemDisplays>,
        rolls: RollCatalogs<'a>,
    ) -> Self {
        let entry = fields.object_entry().unwrap_or(0);
        LiveItem {
            entry,
            object: Some((fields, guid)),
            template: (entry != 0)
                .then(|| items.template(entry, guid, Ask::Read))
                .flatten(),
            roll: ItemRoll::default(),
            items,
            icons,
            rolls,
            classes: ClassNames::default(),
        }
    }

    /// The record `entry` alone, as a slot that carries `roll` shows it.
    pub(crate) fn record(
        entry: u32,
        roll: ItemRoll,
        items: &'a Items,
        icons: Option<&'a ItemDisplays>,
        rolls: RollCatalogs<'a>,
    ) -> Self {
        LiveItem {
            entry,
            object: None,
            template: items.template(entry, 0, Ask::Read),
            roll,
            items,
            icons,
            rolls,
            classes: ClassNames::default(),
        }
    }

    /// The roll the link and the name carry: an object's own (`0x52ae00`, none when wrapped),
    /// else the slot's.
    fn roll(&self) -> ItemRoll {
        match self.object {
            Some((fields, _)) => {
                let (enchant, property, suffix_factor) =
                    super::object_link_numbers(fields, self.template);
                ItemRoll {
                    enchant,
                    property,
                    suffix_factor,
                }
            }
            None => self.roll,
        }
    }
}

impl ItemRead for LiveItem<'_> {
    fn cached(&self) -> bool {
        self.template.is_some()
    }

    fn name(&self) -> Option<String> {
        // `CGItem_C::GetFullName` (`0x5d8bc0`): the record's name with the roll's suffix
        // (`0x5d8b00`), none on a wrapped object.
        let t = self.template?;
        Some(self.rolls.name(&t.name, self.roll().property))
    }

    fn texture(&self) -> Option<String> {
        // The record's display, never the wire's (`0x5d88b0([rec+0x18])`, `0x5d8af0`).
        let t = self.template?;
        self.icons?.catalog.get(t.display_info_id)?.icon.clone()
    }

    fn quality(&self) -> Option<u32> {
        self.template.map(|t| t.quality)
    }

    fn inventory_type(&self) -> u32 {
        self.template.map_or(0, |t| t.inventory_type)
    }

    fn class(&self) -> Option<u32> {
        self.template.map(|t| t.class)
    }

    fn link(&self) -> Option<String> {
        // `0x52ae00` for an object, the link every bag, worn and own trade slot makes of the item
        // it holds (`0x4f997f`, `0x4c8ce2`, `0x4c06bc`): its record's quality and full name.
        let t = self.template?;
        let r = self.roll();
        Some(super::item_link_full(
            self.entry,
            r.enchant,
            r.property,
            r.suffix_factor,
            &self.rolls.name(&t.name, r.property),
            t.quality,
        ))
    }

    fn uncached_link(&self) -> Option<String> {
        // With no record the link is white (`0x52aec5`) and nameless (`0x5d8b40`), and the
        // record's flags read 0 (`0x5da2a0`), so the enchant goes in. An object with no entry yet
        // links nothing, as its slot shows only its lock.
        if self.template.is_some() || self.object.is_none() || self.entry == 0 {
            return None;
        }
        let r = self.roll();
        Some(super::item_link_full(
            self.entry,
            r.enchant,
            r.property,
            r.suffix_factor,
            "",
            1,
        ))
    }

    fn stack_count(&self) -> Option<u32> {
        self.object.map(|(f, _)| f.item_stack_count().unwrap_or(1))
    }

    fn durability(&self) -> Option<(u32, u32)> {
        // Max 0 is indestructible.
        let (f, _) = self.object?;
        f.item_durability()
            .zip(f.item_max_durability())
            .filter(|&(_, max)| max > 0)
    }

    fn info(&self) -> Option<benilla_ui::script::ItemInfoFields> {
        let t = self.template?;
        Some(benilla_ui::script::ItemInfoFields {
            required_level: t.required_level,
            // The stack size; `max_count` is the unique cap.
            stackable: t.stackable,
            item_type: self.classes.item_type(t),
            item_sub_type: self.classes.item_sub_type(t),
        })
    }

    fn readable(&self) -> bool {
        // The cached record's PageText (`0x5d9e10`), 0 while uncached, else the instance's letter
        // text (`0x4f97c1`).
        let guid = self.object.map_or(0, |(_, g)| g);
        crate::items::record_page_text(self.items, self.entry, guid) != 0
            || self
                .object
                .is_some_and(|(f, _)| f.item_text_id().is_some_and(|id| id != 0))
    }
}

/// What the call host reads an item off as a getter runs ([`LiveItem`]).
#[derive(SystemParam)]
pub(crate) struct ItemLive<'w, 's> {
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    rolls: crate::items::Rolls<'w>,
    objects: crate::net::Objects<'w, 's>,
    classes: Option<Res<'w, super::ItemClasses>>,
    sub_classes: Option<Res<'w, super::ItemSubClasses>>,
}

impl ItemLive<'_, '_> {
    /// Hand `read` the item `key` names; whether it was held.
    pub(crate) fn read(&self, key: ItemKey, read: &mut dyn FnMut(&dyn ItemRead)) -> bool {
        let (items, icons, rolls) = (&*self.items, self.icons.as_deref(), self.rolls.get());
        let classes = ClassNames {
            classes: self.classes.as_deref().map(|c| &c.0),
            sub_classes: self.sub_classes.as_deref().map(|c| &c.0),
        };
        match key {
            ItemKey::Object(guid) => {
                let Some(fields) = self.objects.object(guid) else {
                    return false;
                };
                read(&LiveItem::object(fields, guid, items, icons, rolls).with_classes(classes));
            }
            ItemKey::Record { entry, roll } => {
                read(&LiveItem::record(entry, roll, items, icons, rolls).with_classes(classes));
            }
        }
        true
    }
}
