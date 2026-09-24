//! `ServerMessages.dbc`, the five sentences the server can say to everyone. `SMSG_SERVER_MESSAGE`
//! carries a row id and a fill: the client indexes this table by `messageType` (`0x49dfbb`, store
//! `[0xc0d990]`) and fills the row's `%s` with the packet's text (`snprintf(buf, 0x400, …)` at
//! `0x49dff0`), or copies the row when the text is empty (`0x49dfc8`, `0x64a5a0`). The ids are
//! vmangos's `ServerMessageType` (`World.h:62`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const SERVER_MESSAGES: &str = "DBFilesClient\\ServerMessages.dbc";

/// The five rows, keyed by id.
#[derive(Clone, Debug, Default)]
pub struct ServerMessagesCatalog {
    rows: Vec<(u32, String)>,
}

impl ServerMessagesCatalog {
    /// A catalog of given rows, for tests without an install.
    pub fn from_rows(rows: Vec<(u32, String)>) -> Self {
        ServerMessagesCatalog { rows }
    }

    /// The format string for a `messageType`.
    pub fn text(&self, message_type: u32) -> Option<&str> {
        self.rows
            .iter()
            .find(|(id, _)| *id == message_type)
            .map(|(_, t)| t.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// The line `SMSG_SERVER_MESSAGE` becomes (`0x49dfab`-`0x49e030`): a row and a fill format, a
    /// row alone is copied, and no row falls back to the reference's `"[%d]: %s"` (`0x844864`).
    ///
    /// Deviation: a row with two `%s` fills only the first, because the reference's one-argument
    /// `snprintf` fills the second with stack garbage; no shipped row has two.
    pub fn compose(&self, message_type: u32, fill: &str) -> String {
        match self.text(message_type) {
            Some(row) if !fill.is_empty() => row.replacen("%s", fill, 1),
            Some(row) => row.to_string(),
            None => format!("[{message_type}]: {fill}"),
        }
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("ServerMessages");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new_array("Text", FieldType::String, 8));
    s.add_field(SchemaField::new("TextMask", FieldType::UInt32));
    s.set_key_field("ID");
    s
}

pub fn load_server_messages_catalog(chain: &mut Chain) -> Result<ServerMessagesCatalog> {
    let bytes = chain
        .read_file(SERVER_MESSAGES)
        .context("reading ServerMessages.dbc")?;
    let rs = parse(&bytes, schema(), "ServerMessages")?;
    let mut rows = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(text)) = (u32_at(r, 0), str_at(&rs, r, 1)) else {
            continue;
        };
        rows.push((id, text));
    }
    Ok(ServerMessagesCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_table_is_the_five_documented_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_server_messages_catalog(&mut chain).expect("load ServerMessages");
        let got: Vec<(u32, &str)> = cat.rows.iter().map(|(i, t)| (*i, t.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (1, "[SERVER] Shutdown in %s"),
                (2, "[SERVER] Restart in %s"),
                (3, "%s"),
                (4, "[SERVER] Shutdown cancelled"),
                (5, "[SERVER] Restart cancelled"),
            ]
        );
    }

    fn shipped() -> ServerMessagesCatalog {
        ServerMessagesCatalog::from_rows(vec![
            (1, "[SERVER] Shutdown in %s".into()),
            (2, "[SERVER] Restart in %s".into()),
            (3, "%s".into()),
            (4, "[SERVER] Shutdown cancelled".into()),
            (5, "[SERVER] Restart cancelled".into()),
        ])
    }

    #[test]
    fn a_countdown_fills_its_row() {
        assert_eq!(
            shipped().compose(1, "15 Minutes"),
            "[SERVER] Shutdown in 15 Minutes"
        );
        assert_eq!(shipped().compose(3, "back in five"), "back in five");
    }

    #[test]
    fn a_cancellation_copies_its_row_whole() {
        assert_eq!(shipped().compose(5, ""), "[SERVER] Restart cancelled");
    }

    #[test]
    fn an_unknown_type_falls_back_to_the_bracketed_form() {
        assert_eq!(shipped().compose(9, "something new"), "[9]: something new");
    }

    /// The reference tests the fill, not the row: a `%s` row with no fill is copied, token and all.
    #[test]
    fn an_empty_fill_never_formats() {
        assert_eq!(shipped().compose(1, ""), "[SERVER] Shutdown in %s");
    }
}
