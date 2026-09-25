//! The auction house's mail: a notice's subject and body are colon-separated text the client
//! parses back, the subject in `0x4ace70` and the body in `GetInboxInvoiceInfo` (`0x4af360`).
//!
//! The subject `<itemEntry>:<randomProperty>:<resultCode>` names the outcome and is displayed
//! through the matching `AUCTION_*_MAIL_SUBJECT` string, never raw. Only won and sold notices carry
//! a body, the invoice's numbers, fetched when the letter is opened.

/// The subject's `resultCode` (vmangos `Mail.h:102`); the client bounds it to `0..=5` for a
/// six-entry table (`0x4acf62`, `0x4acfec`).
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
    /// The random-property roll. Parsed, not applied: the reference names the item with it
    /// (`[rec+0x250]`, `[rec+0x254]`), and the displayed subject here does not.
    #[allow(dead_code)]
    pub(crate) random_property: i32,
    pub(crate) result: u32,
}

/// Parses `<itemEntry>:<randomProperty>:<resultCode>` strictly: on top of the type byte, the
/// reference also refuses a subject whose fields do not all parse in range.
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

/// The `AUCTION_*_MAIL_SUBJECT` key a result code displays under (`0x4acfec`); the two cancel
/// codes share one. The strings are the install's own (`GlobalStrings.lua:83-99`).
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
    /// The counterparty's guid, hex right-aligned in a 16-wide field, so it may lead with spaces.
    pub(crate) player_guid: u64,
    pub(crate) bid: u32,
    pub(crate) buyout: u32,
    /// Seller bodies only.
    pub(crate) deposit: u32,
    /// Seller bodies only: the auction house's cut.
    pub(crate) consignment: u32,
}

/// Parses an invoice body with the reference's `sscanf` formats, `%16I64X:%d:%d:%d:%d` for a
/// seller and `%16I64X:%d:%d` for a buyer. Deviation: a wrong field count is no invoice, because
/// the reference would read the missing fields uninitialised.
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

    /// Two subjects as the live server sends them.
    #[test]
    fn the_subject_triplet_names_the_outcome() {
        let won = parse_subject("4428:0:1").expect("won");
        assert_eq!((won.entry, won.result), (4428, auction_mail::WON));
        assert_eq!(subject_key(won.result), Some("AUCTION_WON_MAIL_SUBJECT"));

        let sold = parse_subject("5529:0:2").expect("sold");
        assert_eq!((sold.entry, sold.result), (5529, auction_mail::SOLD));
        assert_eq!(subject_key(sold.result), Some("AUCTION_SOLD_MAIL_SUBJECT"));

        // Both cancel codes share one key, as in the reference's table.
        assert_eq!(
            subject_key(auction_mail::CANCELLED_TO_BIDDER),
            subject_key(auction_mail::CANCELLED)
        );
    }

    #[test]
    fn a_subject_that_is_not_the_triplet_is_not_an_auction_notice() {
        assert_eq!(parse_subject("Hi there"), None);
        assert_eq!(parse_subject("4428:0"), None, "two fields");
        assert_eq!(parse_subject("4428:0:1:9"), None, "four fields");
        assert_eq!(parse_subject("4428:0:6"), None, "result code out of range");
        assert_eq!(parse_subject(""), None);
    }

    /// Two bodies as the live server sends them.
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

        // A body asked for as the other kind answers nothing.
        assert_eq!(parse_body("6C:10000:10000", true), None);
        assert_eq!(parse_body("6C:10000:10000:25:500", false), None);
    }

    /// vmangos pads the guid to 16 with spaces (`AuctionHouseMgr.cpp:171`); the reference's
    /// `sscanf` skips them.
    #[test]
    fn a_space_padded_guid_still_parses() {
        let b = parse_body("              6C:10000:10000", false).expect("padded");
        assert_eq!(b.player_guid, 0x6C);
    }
}
