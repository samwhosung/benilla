//! The addon-info block ending `CMSG_AUTH_SESSION`, written by `0x51d910` after the 20-byte
//! proof: a `u32` uncompressed size, then a zlib (RFC1950) stream to the end of the packet.
//! Uncompressed, it is one record per addon whose `.toc` sets `## Secure:` non-zero, enabled or
//! not, with no count or trailer: `CString` name, `u8` flags (`.pub` byte 0), `u32` CRC-32 of the
//! 256 modulus bytes (`.pub` bytes 1..=256), `u32` CRC-32 of the `.url` string (0 if none).
//!
//! With no secure addons the client appends nothing. Both emulators refuse a zero size: vmangos
//! then skips `SMSG_ADDON_INFO` (`WorldSocket.cpp:447`), cmangos-classic kicks the session
//! (`WorldSocket.cpp:562`).

/// CRC-32 of the stock Blizzard public-key modulus, the emulators' "standard addon CRC"; a server
/// seeing it does not send the 256-byte modulus back in `SMSG_ADDON_INFO`.
pub const STANDARD_MODULUS_CRC: u32 = 0x4C1C_776D;

/// One record of `SMSG_ADDON_INFO` (the handler `0x51da70`), in the order of the addon block we
/// sent. The reply carries no names, so [`pair_with_sent`] gives each its addon's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddonRecord {
    /// The addon this record answers (empty until paired).
    pub name: String,
    /// `0` bans the addon (`[rec+0x24] = 2`, `0x51dc0a`); `2` hides it from the Lua index
    /// (`[rec+0x29] = 1`, `0x51db84`); any other value leaves it listed. The signature check runs
    /// for every status but `0`.
    pub status: u8,
    /// `infoProvided` (`[rec+0xc1]`): a key and a revision follow.
    pub info_provided: bool,
    /// A 256-byte modulus followed (`[rec+0xc2]`), which the reference saves to `<Name>.pub`.
    pub key_provided: bool,
    /// The server's revision (`0x418eb0`); above the installed `## Revision` (`[rec+0x20]`) it
    /// marks a new version (`[rec+0xc0] = 1`, `0x51db0d`).
    pub revision: Option<u32>,
    /// The `urlProvided` string (`[rec+0x30]`, `0x51db6a`), up to its first NUL.
    pub url: Option<String>,
}

/// Name each record of the reply with the addon we sent in the same position: the reply has no
/// names, and the reference walks its own secure addons in the order it sent them (`0x51da70`).
/// A record past the end of what we sent answers nothing and is dropped.
///
/// The reference walks the addons, not the records: when the packet runs out, the readers
/// (`0x418cb0`) leave their outputs alone and the status local keeps the last record's value, so
/// every addon past the last record takes that status, with no info block and no url.
pub fn pair_with_sent(records: &[AddonRecord], sent: &[SecureAddon]) -> Vec<AddonRecord> {
    let mut status = None;
    sent.iter()
        .enumerate()
        .filter_map(|(i, addon)| {
            let record = match records.get(i) {
                Some(record) => {
                    status = Some(record.status);
                    record.clone()
                }
                None => AddonRecord {
                    status: status?,
                    ..AddonRecord::default()
                },
            };
            Some(AddonRecord {
                name: addon.name.to_string(),
                ..record
            })
        })
        .collect()
}

/// One record of the addon block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecureAddon<'a> {
    /// The addon's folder name (`Blizzard_AuctionUI`), sent as a `CString`.
    pub name: &'a str,
    /// Byte 0 of the `.pub`: 1 for a stock signature, 0 for none usable. A server may write another
    /// back in `SMSG_ADDON_INFO`, which the reference saves to the `.pub` and echoes next logon.
    pub flags: u8,
    /// CRC-32 of the `.pub`'s 256 modulus bytes; 0 when `flags` is 0.
    pub modulus_crc: u32,
    /// CRC-32 of the addon's `.url` string. `0` for every stock addon (none ship one).
    pub url_crc: u32,
}

impl SecureAddon<'_> {
    /// A stock signed built-in, the shape all twelve take on an unmodified install.
    const fn stock(name: &str) -> SecureAddon<'_> {
        SecureAddon {
            name,
            flags: 1,
            modulus_crc: STANDARD_MODULUS_CRC,
            url_crc: 0,
        }
    }
}

/// The twelve `Blizzard_*` built-ins a stock 1.12.1 install reports, in the client's order
/// (ascending ASCII). Deviation: a fixed list, not read off the install, because no third-party
/// addon sets `## Secure:`, so the two differ only where the `.pub` files were altered.
pub const STOCK_SECURE_ADDONS: [SecureAddon<'static>; 12] = [
    SecureAddon::stock("Blizzard_AuctionUI"),
    SecureAddon::stock("Blizzard_BattlefieldMinimap"),
    SecureAddon::stock("Blizzard_BindingUI"),
    SecureAddon::stock("Blizzard_CombatText"),
    SecureAddon::stock("Blizzard_CraftUI"),
    SecureAddon::stock("Blizzard_GMSurveyUI"),
    SecureAddon::stock("Blizzard_InspectUI"),
    SecureAddon::stock("Blizzard_MacroUI"),
    SecureAddon::stock("Blizzard_RaidUI"),
    SecureAddon::stock("Blizzard_TalentUI"),
    SecureAddon::stock("Blizzard_TradeSkillUI"),
    SecureAddon::stock("Blizzard_TrainerUI"),
];

/// The uncompressed addon buffer: the records concatenated, nothing else.
pub fn addon_block(addons: &[SecureAddon]) -> Vec<u8> {
    let mut out = Vec::with_capacity(addons.iter().map(|a| a.name.len() + 10).sum());
    for addon in addons {
        out.extend_from_slice(addon.name.as_bytes());
        out.push(0);
        out.push(addon.flags);
        out.extend_from_slice(&addon.modulus_crc.to_le_bytes());
        out.extend_from_slice(&addon.url_crc.to_le_bytes());
    }
    out
}

/// The block on the wire: `u32` size and the zlib stream, or nothing when there are no addons.
pub fn addon_tail(addons: &[SecureAddon]) -> Vec<u8> {
    if addons.is_empty() {
        return Vec::new();
    }
    let plain = addon_block(addons);
    let mut out = Vec::with_capacity(plain.len() / 2 + 8);
    out.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    let mut encoder =
        flate2::write::ZlibEncoder::new(out, flate2::Compression::default() /* level 6 */);
    std::io::Write::write_all(&mut encoder, &plain).expect("zlib write to a Vec cannot fail");
    encoder.finish().expect("zlib finish on a Vec cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The block has retail's 342-byte length and parses as twelve sorted, stock-signed records.
    #[test]
    fn stock_block_has_the_retail_size_and_twelve_sorted_stock_records() {
        let plain = addon_block(&STOCK_SECURE_ADDONS);
        assert_eq!(plain.len(), 342, "retail sent 342 uncompressed bytes");

        // Parse it as a server does; landing exactly on the end proves no count and no trailer.
        let mut rest = &plain[..];
        let mut seen = Vec::new();
        while !rest.is_empty() {
            let nul = rest
                .iter()
                .position(|&b| b == 0)
                .expect("record has a name");
            let name = std::str::from_utf8(&rest[..nul]).expect("name is utf-8");
            rest = &rest[nul + 1..];
            let (flags, tail) = rest.split_first().expect("record has flags");
            let modulus_crc = u32::from_le_bytes(tail[0..4].try_into().unwrap());
            let url_crc = u32::from_le_bytes(tail[4..8].try_into().unwrap());
            assert_eq!(*flags, 1, "{name} is a stock signed addon");
            assert_eq!(modulus_crc, STANDARD_MODULUS_CRC, "{name} modulus crc");
            assert_eq!(url_crc, 0, "{name} ships no .url");
            seen.push(name.to_string());
            rest = &tail[8..];
        }
        assert_eq!(seen.len(), 12);
        assert!(seen.iter().all(|n| n.starts_with("Blizzard_")));
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "the client sends them in ascending ASCII order"
        );
    }

    /// `flate2`'s default is the reference's `Z_DEFAULT_COMPRESSION`, hence the retail 130 bytes.
    #[test]
    fn stock_tail_is_size_plus_zlib() {
        let tail = addon_tail(&STOCK_SECURE_ADDONS);
        assert_eq!(
            u32::from_le_bytes(tail[0..4].try_into().unwrap()),
            342,
            "size prefix is the uncompressed length"
        );
        assert_eq!(tail[4], 0x78, "RFC1950 zlib framing, not raw deflate");
        assert_eq!(tail.len(), 4 + 130, "retail compressed to 130 bytes");
    }

    #[test]
    fn no_secure_addons_appends_nothing() {
        assert!(addon_tail(&[]).is_empty());
    }
}
