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
    /// The random-property roll, which names the item's suffix in the subject and the invoice
    /// (`[rec+0x254]`, `0x5d8b00`).
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

/// Reads an invoice body as `GetInboxInvoiceInfo` does: its outputs zeroed (`0x4af440`-`0x4af44f`),
/// then `sscanf` (`0x73fb5d`) with `%16I64X:%d:%d:%d:%d` for a seller (`0x846418`, `0x4af471`) or
/// `%16I64X:%d:%d` for a buyer (`0x846408`, `0x4af495`), whose count is never tested
/// (`0x4af476`): the fields are read in order, and the first that fails leaves itself and the
/// rest at 0.
pub(crate) fn parse_body(body: &str, seller: bool) -> InvoiceNumbers {
    let mut n = InvoiceNumbers::default();
    let mut scan = Scan(body.as_bytes());
    let _ = (|| -> Option<()> {
        n.player_guid = scan.hex64(16)?;
        scan.literal(b':')?;
        n.bid = scan.decimal()?;
        scan.literal(b':')?;
        n.buyout = scan.decimal()?;
        if seller {
            scan.literal(b':')?;
            n.deposit = scan.decimal()?;
            scan.literal(b':')?;
            n.consignment = scan.decimal()?;
        }
        Some(())
    })();
    n
}

/// The CRT's `_input` over the body's bytes, for the two conversions the invoice formats use.
struct Scan<'a>(&'a [u8]);

impl Scan<'_> {
    fn peek(&self) -> Option<u8> {
        self.0.first().copied()
    }

    fn bump(&mut self) {
        self.0 = &self.0[1..];
    }

    /// A conversion first skips `isspace` bytes: space and `\t` to `\r`.
    fn skip_space(&mut self) {
        while self
            .peek()
            .is_some_and(|c| c == b' ' || (b'\t'..=b'\r').contains(&c))
        {
            self.bump();
        }
    }

    /// A literal format byte matches the next input byte, with no space skipped; a mismatch ends
    /// the scan.
    fn literal(&mut self, c: u8) -> Option<()> {
        (self.peek() == Some(c)).then(|| self.bump())
    }

    /// An optional sign, counted against `width`; `true` when it was `-`.
    fn sign(&mut self, width: &mut usize) -> bool {
        match self.peek() {
            Some(c @ (b'-' | b'+')) => {
                self.bump();
                *width = width.saturating_sub(1);
                c == b'-'
            }
            _ => false,
        }
    }

    /// `%<width>I64X`: a sign, an optional `0x` or `0X` that the width does not count
    /// (`0x745324`-`0x74537c`), then hex digits up to the width; none is a failed conversion. A
    /// `-` negates.
    fn hex64(&mut self, mut width: usize) -> Option<u64> {
        self.skip_space();
        let negative = self.sign(&mut width);
        if self.peek() == Some(b'0') && matches!(self.0.get(1), Some(b'x' | b'X')) {
            self.bump();
            self.bump();
        }
        let mut n = 0u64;
        let mut digits = 0;
        while digits < width {
            let Some(d) = self.peek().and_then(|c| char::from(c).to_digit(16)) else {
                break;
            };
            n = n << 4 | u64::from(d);
            self.bump();
            digits += 1;
        }
        (digits > 0).then(|| if negative { n.wrapping_neg() } else { n })
    }

    /// `%d`: a sign, then decimal digits with no width, wrapping in 32 bits as the CRT's `long`
    /// does; none is a failed conversion. The invoice reads the result as unsigned.
    fn decimal(&mut self) -> Option<u32> {
        self.skip_space();
        let mut unbounded = usize::MAX;
        let negative = self.sign(&mut unbounded);
        let mut n = 0u32;
        let mut digits = 0;
        while let Some(d) = self.peek().and_then(|c| char::from(c).to_digit(10)) {
            n = n.wrapping_mul(10).wrapping_add(d);
            self.bump();
            digits += 1;
        }
        (digits > 0).then(|| if negative { n.wrapping_neg() } else { n })
    }
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
        let sold = parse_body("6C:10000:10000:25:500", true);
        assert_eq!(sold.player_guid, 0x6C, "hex, not decimal");
        assert_eq!((sold.bid, sold.buyout), (10000, 10000));
        assert_eq!((sold.deposit, sold.consignment), (25, 500));

        let won = parse_body("6C:10000:10000", false);
        assert_eq!((won.player_guid, won.bid, won.buyout), (0x6C, 10000, 10000));
        assert_eq!(
            (won.deposit, won.consignment),
            (0, 0),
            "not in a buyer body"
        );

        // A body read as the other kind takes what the format reaches.
        let short = parse_body("6C:10000:10000", true);
        assert_eq!((short.bid, short.deposit, short.consignment), (10000, 0, 0));
        let long = parse_body("6C:10000:10000:25:500", false);
        assert_eq!((long.buyout, long.deposit), (10000, 0));
    }

    /// vmangos pads the guid to 16 with spaces (`AuctionHouseMgr.cpp:171`); the reference's
    /// `sscanf` skips them.
    #[test]
    fn a_space_padded_guid_still_parses() {
        let b = parse_body("              6C:10000:10000", false);
        assert_eq!(b.player_guid, 0x6C);
    }

    /// `sscanf`'s rules over the two conversions: space skipped before each, not before a literal;
    /// `%16I64X` stops at 16 digits, so a 17th meets the `:`; `%d` takes a sign and wraps in 32 bits;
    /// the first failure leaves the rest at 0.
    #[test]
    fn the_body_reads_as_sscanf_does() {
        let n = |body: &str| {
            let b = parse_body(body, true);
            (b.player_guid, b.bid, b.buyout, b.deposit, b.consignment)
        };
        assert_eq!(n("42: 7:\t-1:+3: 4"), (0x42, 7, u32::MAX, 3, 4));
        assert_eq!(
            n("42 :7:8:9:10"),
            (0x42, 0, 0, 0, 0),
            "no space before a literal"
        );
        assert_eq!(
            n("0000000000000000042:7:8:9:10"),
            (0, 0, 0, 0, 0),
            "16 digits, then a 17th where `:` must be"
        );
        assert_eq!(n("0x42:7:8:9:10"), (0x42, 7, 8, 9, 10), "a 0x prefix");
        assert_eq!(
            n("0X123456789ABCDEF0:7:8:9:10"),
            (0x1234_5678_9ABC_DEF0, 7, 8, 9, 10),
            "the prefix takes none of the 16"
        );
        assert_eq!(n("-1:4294967296:9:x:1"), (u64::MAX, 0, 9, 0, 0));
        assert_eq!(n("42:7:8"), (0x42, 7, 8, 0, 0), "partial");
        assert_eq!(n("garbage"), (0, 0, 0, 0, 0));
        assert_eq!(n(""), (0, 0, 0, 0, 0));
    }
}
