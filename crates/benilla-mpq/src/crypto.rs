//! The Storm (MPQ) crypto, byte-exact with the client: the string hash and the block decryptor,
//! both over one 0x500-entry table. 1.12.1 archives carry no encrypted files, so only the hash and
//! block tables are ever decrypted.

/// The 0x500-entry encryption table, built at compile time by the seed walk the client runs.
const fn encryption_table() -> [u32; 0x500] {
    let mut table = [0u32; 0x500];
    let mut seed: u32 = 0x0010_0001;
    let mut i1 = 0;
    while i1 < 0x100 {
        let mut i2 = 0;
        while i2 < 5 {
            let idx = i1 + i2 * 0x100;
            seed = (seed.wrapping_mul(125).wrapping_add(3)) % 0x2A_AAAB;
            let t1 = (seed & 0xFFFF) << 0x10;
            seed = (seed.wrapping_mul(125).wrapping_add(3)) % 0x2A_AAAB;
            let t2 = seed & 0xFFFF;
            table[idx] = t1 | t2;
            i2 += 1;
        }
        i1 += 1;
    }
    table
}

static TABLE: [u32; 0x500] = encryption_table();

/// Hash-type selectors: the table-quadrant offsets the client uses.
pub mod hash_type {
    /// Hash-table slot index.
    pub const TABLE_OFFSET: u32 = 0x000;
    /// Filename verifier A, stored in the hash entry.
    pub const NAME_A: u32 = 0x100;
    /// Filename verifier B, stored in the hash entry.
    pub const NAME_B: u32 = 0x200;
    /// File and table decryption key.
    pub const FILE_KEY: u32 = 0x300;
}

/// Blizzard's string hash: `/` reads as `\`, and only `a`-`z` are uppercased, as the client's
/// `AsciiToUpper` table does.
pub fn hash_string(name: &str, kind: u32) -> u32 {
    let mut seed1: u32 = 0x7FED_7FED;
    let mut seed2: u32 = 0xEEEE_EEEE;
    for &b in name.as_bytes() {
        let ch = match b {
            b'/' => b'\\',
            other => other.to_ascii_uppercase(),
        } as u32;
        seed1 = TABLE[(kind.wrapping_add(ch)) as usize] ^ seed1.wrapping_add(seed2);
        seed2 = ch
            .wrapping_add(seed1)
            .wrapping_add(seed2)
            .wrapping_add(seed2 << 5)
            .wrapping_add(3);
    }
    seed1
}

/// Decrypt `u32`s in place with `key`; a zero key is a no-op, as in the client.
pub fn decrypt_block(data: &mut [u32], mut key: u32) {
    if key == 0 {
        return;
    }
    let mut seed: u32 = 0xEEEE_EEEE;
    for v in data.iter_mut() {
        seed = seed.wrapping_add(TABLE[0x400 + (key & 0xFF) as usize]);
        let ch = *v ^ key.wrapping_add(seed);
        *v = ch;
        key = (!key << 0x15).wrapping_add(0x1111_1111) | (key >> 0x0B);
        seed = ch
            .wrapping_add(seed)
            .wrapping_add(seed << 5)
            .wrapping_add(3);
    }
}

/// The inverse of [`decrypt_block`], for tests that build the encrypted tables an archive stores.
#[cfg(test)]
pub(crate) fn encrypt_block(data: &mut [u32], mut key: u32) {
    let mut seed: u32 = 0xEEEE_EEEE;
    for v in data.iter_mut() {
        seed = seed.wrapping_add(TABLE[0x400 + (key & 0xFF) as usize]);
        let plain = *v;
        *v = plain ^ key.wrapping_add(seed);
        key = (!key << 0x15).wrapping_add(0x1111_1111) | (key >> 0x0B);
        seed = plain
            .wrapping_add(seed)
            .wrapping_add(seed << 5)
            .wrapping_add(3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_vectors_match_stormlib() {
        assert_eq!(
            hash_string("(hash table)", hash_type::FILE_KEY),
            0xC3AF_3770
        );
        assert_eq!(
            hash_string("(block table)", hash_type::FILE_KEY),
            0xEC83_B3A3
        );
        assert_eq!(
            hash_string("file.txt", hash_type::TABLE_OFFSET),
            0x3EA9_8D7A
        );
    }

    #[test]
    fn hash_is_slash_and_case_insensitive() {
        let a = hash_string("path\\to\\FILE.blp", hash_type::TABLE_OFFSET);
        let b = hash_string("path/to/file.blp", hash_type::TABLE_OFFSET);
        assert_eq!(a, b);
    }

    #[test]
    fn decrypt_inverts_encrypt() {
        let key = hash_string("(hash table)", hash_type::FILE_KEY);
        let original = [0x1234_5678u32, 0x9ABC_DEF0, 0x0F0F_0F0F, 42];
        let mut buf = original;
        encrypt_block(&mut buf, key);
        decrypt_block(&mut buf, key);
        assert_eq!(buf, original);
    }
}
