//! `MailTemplate.dbc`: a quest's or an event's letter, by the mail template id a list row carries.
//! The store is `0xc0dab4` (`0x54a6b0`), 10 columns of `0x28` bytes: the id, the body in eight
//! locales and their flags. `GetInboxText` (`0x4af110`) indexes it by id up to the highest
//! (`[0xc0dac0]`, `0x4af223`) and reads `[row + 4 + 4 * locale]` (`0x4af249`-`0x4af24e`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{loc_str_raw, parse, u32_at};

const MAIL_TEMPLATE: &str = "DBFilesClient\\MailTemplate.dbc";

/// The column count the loader requires.
const MAIL_TEMPLATE_FIELDS: usize = 10;

/// Load `MailTemplate.dbc`'s bodies by id, in the locale's column. A row whose body the file
/// leaves blank answers `""`, as the reference's row holds a pointer to an empty string.
pub fn load_mail_templates(chain: &mut Chain) -> Result<HashMap<u32, String>> {
    let bytes = chain
        .read_file(MAIL_TEMPLATE)
        .context("reading MailTemplate.dbc")?;
    let mut schema = Schema::new("MailTemplate");
    for i in 0..MAIL_TEMPLATE_FIELDS {
        let ty = if (1..=8).contains(&i) {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        schema.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    let set = parse(&bytes, schema, "MailTemplate.dbc")?;
    let locale = chain.locale();
    Ok(set
        .records()
        .iter()
        .filter_map(|r| Some((u32_at(r, 0)?, loc_str_raw(&set, r, 1, locale)?)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Template 87, a fishing quest's reward letter: Gubber Blump's thanks.
    #[test]
    fn the_shipped_templates_load_by_id() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_mail_templates(&mut chain).expect("load MailTemplate.dbc");
        let body = rows.get(&87).expect("template 87");
        assert!(
            body.starts_with("Hello there again.  I'm Gubber Blump."),
            "{body:?}"
        );
        assert_eq!(rows.len(), 96, "the 1.12.1 file's rows");
    }
}
