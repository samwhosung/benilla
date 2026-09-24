//! GMTicketCategory.dbc — the ten trouble-ticket categories the Help window's "page a GM" list is
//! built from, and the id each one submits as.
//!
//! This table is the answer to a question the shipped FrameXML makes look unanswerable. Its
//! `HelpFrameGM_UpdateCategories(GetGMTicketCategories())` consumes the binding's varargs as
//! `(key, text)` PAIRS, and `HELPFRAME_FRAMES`/`GENERAL_HELPFRAME` are keyed `1..10` with ten
//! distinct titles — yet `GlobalStrings.lua` ships only `TICKET_TYPE1..4` ("Game Play",
//! "Harassment", "Stuck", "Bug"). Those four are not this list: they feed the OpenTicket
//! *dropdown*, which has no frame in 1.12 and whose `OnShow` call is commented out. The category
//! list is DBC data, and it lines up with `GENERAL_HELPFRAME`'s keys row for row:
//!
//! | id | name |
//! |---|---|
//! | 1 | Stuck |
//! | 2 | Behavior/Harassment |
//! | 3 | Guild |
//! | 4 | Item |
//! | 5 | Environmental |
//! | 6 | Non-Quest/Creep |
//! | 7 | Quest/Quest NPC |
//! | 8 | Technical |
//! | 9 | Account/Billing |
//! | 10 | Character |
//!
//! **benilla's own Help window no longer shows this list**: it goes straight from
//! Home to the ticket box and files under 0, "uncategorised". The catalog still ships, because
//! `GetGMTicketCategories()` is a real Era binding a third-party addon may call and because these
//! ids are still what the *server* names a ticket by — an existing ticket's category arrives on
//! `UPDATE_TICKET` and is echoed back on an edit.
//!
//! **The id is the wire value**, not just a list index: the clicked button stores it as
//! `HelpFrameOpenTicket.ticketType`, and that is what `NewGMTicket(category, text)` puts in
//! `CMSG_GMTICKET_CREATE`'s category field. So a catalog that renumbered on load would file every
//! ticket under the wrong heading, silently — which is why this is an ordered id-keyed read of the
//! file rather than a `Vec` indexed from zero.
//!
//! Record layout (10 rows in the shipped 5875 file, verified by loading it): `ID@0`,
//! `Name_Lang@1..8`, `NameFlags@9` — the same 8-locale + mask shape as [`crate::itembagfamily`].
//! Ids are 1..10 contiguous here, but nothing downstream may assume that: the ordered pairs are
//! what the binding pushes, and the id is what the wire carries.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const GM_TICKET_CATEGORY: &str = "DBFilesClient\\GMTicketCategory.dbc";

/// GMTicketCategory.dbc, in file order — the order `GetGMTicketCategories()` pushes them in, which
/// is the order the ten `HelpFrameButton*` rows are painted in.
pub struct GmTicketCategoryCatalog {
    categories: Vec<GmTicketCategory>,
}

/// One row: the wire category id and its localized display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmTicketCategory {
    /// The id `CMSG_GMTICKET_CREATE` carries — see the module doc's warning about renumbering.
    pub id: u32,
    /// The button label, from the active locale's `Name_Lang` slot.
    pub name: String,
}

impl GmTicketCategoryCatalog {
    /// The rows, in file order.
    pub fn categories(&self) -> &[GmTicketCategory] {
        &self.categories
    }

    /// Row count, for the load log.
    pub fn len(&self) -> usize {
        self.categories.len()
    }

    pub fn is_empty(&self) -> bool {
        self.categories.is_empty()
    }
}

fn gm_ticket_category_schema() -> Schema {
    let mut s = Schema::new("GMTicketCategory");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// Load GMTicketCategory.dbc from the patch chain.
///
/// A row whose name is empty is skipped rather than pushed blank: the Help window paints one
/// button per returned pair and an unnamed button is a dead click, not a category. The shipped
/// 5875 file has no such row — this is a guard on a locale slot, not a modelled branch.
pub fn load_gm_ticket_categories(chain: &mut Chain) -> Result<GmTicketCategoryCatalog> {
    let bytes = chain
        .read_file(GM_TICKET_CATEGORY)
        .with_context(|| format!("reading {GM_TICKET_CATEGORY}"))?;
    let rs = parse(&bytes, gm_ticket_category_schema(), "GMTicketCategory")?;
    let mut categories = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(name)) = (u32_at(r, 0), str_at(&rs, r, 1).filter(|n| !n.is_empty()))
        else {
            continue;
        };
        categories.push(GmTicketCategory { id, name });
    }
    Ok(GmTicketCategoryCatalog { categories })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped 5875 table, read as data rather than assumed. Pins **both** halves that matter:
    /// the ids (which go on the wire) and the order (which is the painted order), plus the row-for-
    /// row correspondence with `GENERAL_HELPFRAME`'s ten keys that the module doc tabulates.
    /// Skips without client data.
    #[test]
    fn the_shipped_categories_are_the_ten_help_window_rows_in_order() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_gm_ticket_categories(&mut chain).expect("GMTicketCategory.dbc");

        let rows: Vec<(u32, &str)> = cat
            .categories()
            .iter()
            .map(|c| (c.id, c.name.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (1, "Stuck"),
                (2, "Behavior/Harassment"),
                (3, "Guild"),
                (4, "Item"),
                (5, "Environmental"),
                (6, "Non-Quest/Creep"),
                (7, "Quest/Quest NPC"),
                (8, "Technical"),
                (9, "Account/Billing"),
                (10, "Character"),
            ],
            "the wire ids and the painted order both matter — see the module doc"
        );
        assert_eq!(cat.len(), 10, "the whole shipped table");
    }
}
