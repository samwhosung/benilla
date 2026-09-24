//! The auction house's mail seam — how a mail from the auction house stops being
//! `4428:0:1` / `6C:10000:10000` and becomes "Auction won: Small Blue Pouch" over a receipt.
//!
//! **The invoice is TEXT.** Nothing structured rides the wire: the auction house writes its numbers
//! into the mail's *subject* and *body* as colon-separated fields, and the client parses them back
//! out. That is not an emulator shortcut — it is what the real 1.12 client does (`0x4ace70` for
//! the subject, `GetInboxInvoiceInfo 0x4af360` for the body).
//!
//! Two halves, and they answer different questions from different places:
//!
//! * **The subject says WHAT HAPPENED.** `<itemEntry>:<randomProperty>:<resultCode>` — five outcomes
//!   over six codes. The client parses it, formats the matching `AUCTION_*_MAIL_SUBJECT`
//!   GlobalString with the item's name, and writes the result back over the *displayed* subject, so
//!   the inbox list and the open letter both read it. The raw triplet is never shown.
//! * **The body says HOW MUCH**, and only for the two outcomes that move money: won (`1`) and sold
//!   (`2`). The other four carry no body at all, which is why an outbid notice has no invoice and
//!   why [`parse_body`] is only ever asked about those two.
//!
//! The identity comes from the subject and the numbers come from the body — so a mail can be
//! recognisably "Auction won: X" in the inbox list while its invoice is still unanswerable, because
//! the body is fetched lazily (`CMSG_ITEM_TEXT_QUERY`) when the letter is opened.

/// The mail's `resultCode` — the third subject field. vmangos writes these as `MailAuctionAnswers`;
/// the client bounds the field to `0..=5` and jumps a six-entry table (`0x4acf62`/`0x4acfec`).
pub(crate) mod auction_mail {
    pub(crate) const OUTBID: u32 = 0;
    pub(crate) const WON: u32 = 1;
    pub(crate) const SOLD: u32 = 2;
    pub(crate) const EXPIRED: u32 = 3;
    pub(crate) const CANCELLED_TO_BIDDER: u32 = 4;
    pub(crate) const CANCELLED: u32 = 5;
}

/// A parsed auction-mail subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuctionSubject {
    pub(crate) entry: u32,
    /// The item's random-property roll, for the suffixed name ("of the Bear"). Carried because the
    /// reference composes the displayed name from **both** (`[rec+0x250]` and `[rec+0x254]`);
    /// benilla's item names are unsuffixed today, so nothing reads it yet.
    #[allow(dead_code)]
    pub(crate) random_property: i32,
    pub(crate) result: u32,
}

/// Parse `"<itemEntry>:<randomProperty>:<resultCode>"`.
///
/// Strict on purpose: a mail whose subject is a player's own free text must NOT be mistaken for an
/// auction notice just because it happens to contain colons, so every field has to parse and the
/// result code has to be in range. The type byte already gated us to `MAIL_AUCTION`, but the
/// reference applies this second test on top of it and so do we — the subject is the only thing
/// that distinguishes the five outcomes, and an unparseable one has no outcome at all.
pub(crate) fn parse_subject(subject: &str) -> Option<AuctionSubject> {
    let mut parts = subject.trim().split(':');
    let entry: u32 = parts.next()?.trim().parse().ok()?;
    let random_property: i32 = parts.next()?.trim().parse().ok()?;
    let result: u32 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() || result > auction_mail::CANCELLED {
        return None;
    }
    Some(AuctionSubject {
        entry,
        random_property,
        result,
    })
}

/// The `AUCTION_*_MAIL_SUBJECT` GlobalStrings key one result code displays under — the reference's
/// six-entry dispatch table at `0x4acfec`, where the two cancel codes share a key. Every one of
/// these strings is `"<something>: %s"` with the item name as the fill, and all five ship in the
/// player's own GlobalStrings.lua (l.83-99), so nothing here carries Blizzard's text.
pub(crate) fn subject_key(result: u32) -> Option<&'static str> {
    Some(match result {
        auction_mail::OUTBID => "AUCTION_OUTBID_MAIL_SUBJECT",
        auction_mail::WON => "AUCTION_WON_MAIL_SUBJECT",
        auction_mail::SOLD => "AUCTION_SOLD_MAIL_SUBJECT",
        auction_mail::EXPIRED => "AUCTION_EXPIRED_MAIL_SUBJECT",
        auction_mail::CANCELLED_TO_BIDDER | auction_mail::CANCELLED => {
            "AUCTION_REMOVED_MAIL_SUBJECT"
        }
        _ => return None,
    })
}

/// The numbers an invoice body carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct InvoiceNumbers {
    /// The counterparty's guid — who bought your auction, or who sold you the one you won. Written
    /// as **hex**, right-aligned in a 16-wide field, so it can arrive with leading spaces.
    pub(crate) player_guid: u64,
    pub(crate) bid: u32,
    pub(crate) buyout: u32,
    /// Seller bodies only.
    pub(crate) deposit: u32,
    /// Seller bodies only — the auction house's cut.
    pub(crate) consignment: u32,
}

/// Parse an invoice body: five fields for a `seller` invoice, three for a buyer's.
///
/// The reference picks the `sscanf` format off the invoice type rather than counting fields
/// (`"%16I64X:%d:%d:%d:%d"` vs `"%16I64X:%d:%d"`), which is why the count is checked here and not
/// inferred: a seller body that arrived three-field would leave the reference reading two
/// uninitialised numbers, and we would rather answer "no invoice" than a deposit of whatever was on
/// the stack.
pub(crate) fn parse_body(body: &str, seller: bool) -> Option<InvoiceNumbers> {
    let fields: Vec<&str> = body.trim().split(':').map(str::trim).collect();
    if fields.len() != if seller { 5 } else { 3 } {
        return None;
    }
    let player_guid = u64::from_str_radix(fields[0], 16).ok()?;
    let bid = fields[1].parse().ok()?;
    let buyout = fields[2].parse().ok()?;
    let (deposit, consignment) = if seller {
        (fields[3].parse().ok()?, fields[4].parse().ok()?)
    } else {
        (0, 0)
    };
    Some(InvoiceNumbers {
        player_guid,
        bid,
        buyout,
        deposit,
        consignment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two subjects the director photographed, byte for byte off the live server.
    #[test]
    fn the_subject_triplet_names_the_outcome() {
        let won = parse_subject("4428:0:1").expect("won");
        assert_eq!((won.entry, won.result), (4428, auction_mail::WON));
        assert_eq!(subject_key(won.result), Some("AUCTION_WON_MAIL_SUBJECT"));

        let sold = parse_subject("5529:0:2").expect("sold");
        assert_eq!((sold.entry, sold.result), (5529, auction_mail::SOLD));
        assert_eq!(subject_key(sold.result), Some("AUCTION_SOLD_MAIL_SUBJECT"));

        // Both cancel codes land on one key (the reference's own table shares the entry).
        assert_eq!(
            subject_key(auction_mail::CANCELLED_TO_BIDDER),
            subject_key(auction_mail::CANCELLED)
        );
    }

    /// A player's own subject line must never be mistaken for an auction notice — the type byte
    /// gates us here, but a hand-typed "3:4:5" would otherwise sail straight through.
    #[test]
    fn a_subject_that_is_not_the_triplet_is_not_an_auction_notice() {
        assert_eq!(parse_subject("Hi there"), None);
        assert_eq!(parse_subject("4428:0"), None, "two fields");
        assert_eq!(parse_subject("4428:0:1:9"), None, "four fields");
        assert_eq!(parse_subject("4428:0:6"), None, "result code out of range");
        assert_eq!(parse_subject(""), None);
    }

    /// The two bodies the director photographed. The seller's five fields carry the arithmetic the
    /// receipt shows; the buyer's three do not, and asking for five is what keeps them apart.
    #[test]
    fn the_body_carries_hex_guid_then_the_money() {
        let sold = parse_body("6C:10000:10000:25:500", true).expect("seller body");
        assert_eq!(sold.player_guid, 0x6C, "hex, not decimal");
        assert_eq!((sold.bid, sold.buyout), (10000, 10000));
        assert_eq!((sold.deposit, sold.consignment), (25, 500));
        // bid == buyout is how the window knows it was bought out rather than bid up.
        assert_eq!(sold.bid, sold.buyout);

        let won = parse_body("6C:10000:10000", false).expect("buyer body");
        assert_eq!(won.player_guid, 0x6C);
        assert_eq!(
            (won.deposit, won.consignment),
            (0, 0),
            "not in a buyer body"
        );

        // The right shape, asked the wrong way round, answers nothing rather than guessing.
        assert_eq!(parse_body("6C:10000:10000", true), None);
        assert_eq!(parse_body("6C:10000:10000:25:500", false), None);
    }

    /// `strm.width(16) << right << hex` pads with spaces, so the guid can arrive with a run of them
    /// in front. The reference's `sscanf` skips leading whitespace; so do we.
    #[test]
    fn a_space_padded_guid_still_parses() {
        let b = parse_body("              6C:10000:10000", false).expect("padded");
        assert_eq!(b.player_guid, 0x6C);
    }
}
