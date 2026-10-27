//! The page-text query pair, the ask-once cache behind every readable book: a `PageText.wdb` id
//! names one page, and each answer carries the next page's id (`0` = last). The first id comes
//! from an item template's `PageText` or a `GAMEOBJECT_TYPE_TEXT` object's `data[0]`; mail bodies
//! use `CMSG_ITEM_TEXT_QUERY` instead.

use std::io;

use crate::wire::{read_cstring_capped, read_u32_le};

/// Body of `CMSG_PAGE_TEXT_QUERY` (`0x005A`): `u32 pageId`, then the asking object's `u64 guid`,
/// which the 1.12 client appends (`0x564730`, at `0x56485d`) and vmangos reads only if present,
/// then discards (`QueryPageText::ReadFromWorldPacket`).
pub fn page_text_query(page_id: u32, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&page_id.to_le_bytes());
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// Read `SMSG_PAGE_TEXT_QUERY_RESPONSE`: `u32 pageId, cstr text, u32 nextPageId`
/// (`PageTextQueryResponse::AppendBodyTo`). vmangos answers one query with the whole chain, one
/// packet per page (`HandlePageTextQueryOpcode`), and an unknown page with `"Item page missing."`
/// and `nextPageId = 0`. The record's read (`0x7cabd0`) takes the text with `GetCString` at
/// capacity `0x400` (`0x7cabda`); a text with no NUL inside that window reads `""` and poisons the
/// store (`0x41929e`-`0x4192be`), so the next id's read fails too (`0x418e30`) and the record
/// keeps 0.
pub(super) fn read_page_text_query_response(r: &mut &[u8]) -> io::Result<(u32, String, u32)> {
    let page_id = read_u32_le(r)?;
    let Some(text) = read_cstring_capped(r, PAGE_TEXT_CAPACITY) else {
        *r = &[];
        return Ok((page_id, String::new(), 0));
    };
    Ok((page_id, text, read_u32_le(r)?))
}

/// The page record's text buffer (`0x7cabda`).
const PAGE_TEXT_CAPACITY: usize = 0x400;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{opcode, parse_server, ServerPacket};

    #[test]
    fn query_body_is_page_id_then_guid() {
        assert_eq!(
            page_text_query(333, 0xF110_0000_0000_0042),
            [
                0x4D, 0x01, 0x00, 0x00, // pageId 333
                0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0xF1, // guid
            ]
        );
    }

    #[test]
    fn response_reads_the_chain_link() {
        let mut body = 333u32.to_le_bytes().to_vec();
        body.extend_from_slice(b"Page one.\0");
        body.extend_from_slice(&334u32.to_le_bytes());
        match parse_server(opcode::SMSG_PAGE_TEXT_QUERY_RESPONSE, &body).unwrap() {
            ServerPacket::PageTextQueryResponse {
                page_id,
                text,
                next_page_id,
            } => {
                assert_eq!(page_id, 333);
                assert_eq!(text, "Page one.");
                assert_eq!(next_page_id, 334);
            }
            other => panic!("expected PageTextQueryResponse, got {}", other.name()),
        }
    }

    /// A last page's zero `nextPageId` reads as 0, the value that ends the chain.
    #[test]
    fn the_last_page_reads_a_zero_next_page_id() {
        let mut body = 9u32.to_le_bytes().to_vec();
        body.extend_from_slice(b"The end.\0");
        body.extend_from_slice(&0u32.to_le_bytes());
        match parse_server(opcode::SMSG_PAGE_TEXT_QUERY_RESPONSE, &body).unwrap() {
            ServerPacket::PageTextQueryResponse { next_page_id, .. } => {
                assert_eq!(next_page_id, 0)
            }
            other => panic!("expected PageTextQueryResponse, got {}", other.name()),
        }
    }

    fn page_of(text: &str) -> (String, u32) {
        let mut body = 9u32.to_le_bytes().to_vec();
        body.extend_from_slice(text.as_bytes());
        body.push(0);
        body.extend_from_slice(&10u32.to_le_bytes());
        match parse_server(opcode::SMSG_PAGE_TEXT_QUERY_RESPONSE, &body).unwrap() {
            ServerPacket::PageTextQueryResponse {
                text, next_page_id, ..
            } => (text, next_page_id),
            other => panic!("expected PageTextQueryResponse, got {}", other.name()),
        }
    }

    /// `GetCString` at `0x400` (`0x7cabda`) holds a text of 1023 bytes and its NUL; one of 1024
    /// has no NUL in the window, so the text reads empty and the poisoned store gives no next id.
    #[test]
    fn a_text_filling_the_buffer_reads_empty_with_no_next_page() {
        assert_eq!(page_of(&"a".repeat(1023)), ("a".repeat(1023), 10));
        assert_eq!(page_of(&"a".repeat(1024)), (String::new(), 0));
        assert_eq!(page_of(&"a".repeat(3000)), (String::new(), 0));
    }
}
