//! Account data wire: the server's digests of the eight sections, a section's download, and the
//! client's request and upload (benilla-re `net/accountdata`, "Server digests" and "Section
//! transfer").

use std::io::{self, Read};

use crate::wire::read_u32_le;

/// The sections the client keeps, `0x85dcf0`'s eight 44-byte records.
pub const ACCOUNT_DATA_SECTIONS: usize = 8;

/// A section's MD5 digest; 16 zero bytes is the server's "no copy".
pub type AccountDataDigest = [u8; 16];

/// `SMSG_ACCOUNT_DATA_MD5` (`0x209`, handler `0x5af8e0`): one 16-byte digest per section, read in
/// section order (`0x5af909`, `GetBytes` of 0x10 per record).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountDataMd5 {
    pub digests: [AccountDataDigest; ACCOUNT_DATA_SECTIONS],
}

pub(super) fn read_account_data_md5(r: &mut impl Read) -> io::Result<AccountDataMd5> {
    let mut digests = [[0u8; 16]; ACCOUNT_DATA_SECTIONS];
    for digest in &mut digests {
        r.read_exact(digest)?;
    }
    Ok(AccountDataMd5 { digests })
}

/// The largest section file the server stores (`MiscHandler.cpp:820`); a download's size past it
/// is no file, so nothing is allocated for it.
const MAX_FILE: u32 = 0xFFFF;

/// `SMSG_UPDATE_ACCOUNT_DATA` (`0x20c`, handler `0x5afc60`): the section (`0x5afc72`), the size of
/// its file (`0x5afc7d`) and a zlib stream to the end of the packet (`0x5afc98`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateAccountData {
    pub section: u32,
    pub size: u32,
    pub stream: Vec<u8>,
}

impl UpdateAccountData {
    /// The section's file, as the handler inflates it into a buffer of `size` bytes (`0x5afca8`,
    /// `0x5afcbc`). `None` for a size of 0, where the handler writes nothing (`0x5afc87`), for one
    /// past [`MAX_FILE`], and for a stream that does not inflate whole within `size`.
    ///
    /// Deviation: a stream that fails to inflate is no file, where the reference writes its
    /// buffer unchecked (`0x5afcbc`), because those bytes are whatever its stack held.
    pub fn contents(&self) -> Option<Vec<u8>> {
        if self.size == 0 || self.size > MAX_FILE {
            return None;
        }
        let mut out = Vec::with_capacity(self.size as usize);
        let mut inflate = flate2::Decompress::new(true);
        match inflate.decompress_vec(&self.stream, &mut out, flate2::FlushDecompress::Finish) {
            Ok(flate2::Status::StreamEnd) => Some(out),
            _ => None,
        }
    }
}

pub(super) fn read_update_account_data(r: &mut impl Read) -> io::Result<UpdateAccountData> {
    let section = read_u32_le(r)?;
    let size = read_u32_le(r)?;
    let mut stream = Vec::new();
    r.read_to_end(&mut stream)?;
    Ok(UpdateAccountData {
        section,
        size,
        stream,
    })
}

/// `CMSG_REQUEST_ACCOUNT_DATA` (`0x20a`, `0x5afbe0`): the section as a `u32` (`0x5afc13`).
pub fn request_account_data(section: u32) -> Vec<u8> {
    section.to_le_bytes().to_vec()
}

/// `CMSG_UPDATE_ACCOUNT_DATA` (`0x20b`, `0x5afae0`): the section (`0x5afb8a`), the file's size
/// (`0x5afb93`), then the file compressed with zlib at level 1 (`0x5afb4c`, `0x5afb55`) to the end
/// (`0x5afba0`). An empty file sends no stream: the reference's compress into a buffer of the
/// file's size has no room and leaves its length 0.
///
/// Deviation: the whole stream is sent, where the reference cuts it at the file's size
/// (`0x5afb42`), because the server refuses a cut stream and the next login then downloads its old
/// copy over the player's file.
pub fn update_account_data(section: u32, contents: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + contents.len() / 2);
    out.extend_from_slice(&section.to_le_bytes());
    out.extend_from_slice(&(contents.len() as u32).to_le_bytes());
    if contents.is_empty() {
        return out;
    }
    let mut encoder = flate2::write::ZlibEncoder::new(out, flate2::Compression::new(1));
    std::io::Write::write_all(&mut encoder, contents).expect("zlib write to a Vec cannot fail");
    encoder.finish().expect("zlib finish on a Vec cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handler reads eight 16-byte digests back to back (`0x5af8f3`-`0x5af924`).
    #[test]
    fn smsg_account_data_md5_body_golden() {
        let mut body = Vec::new();
        for section in 0..8u8 {
            body.extend((0..16u8).map(|i| section * 0x10 + i));
        }
        let md5 = read_account_data_md5(&mut body.as_slice()).unwrap();
        assert_eq!(
            md5.digests[0],
            *b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f"
        );
        assert_eq!(md5.digests[7][0], 0x70);
        assert_eq!(md5.digests[7][15], 0x7f);
        assert!(
            read_account_data_md5(&mut &body[..127]).is_err(),
            "a short digest table is no table"
        );
        assert!(matches!(
            crate::messages::parse_server(crate::messages::opcode::SMSG_ACCOUNT_DATA_MD5, &body),
            Ok(crate::messages::ServerPacket::AccountDataMd5(m)) if m == md5
        ));
    }

    /// `0x5afbe0` puts the opcode, then the section as one `PutUInt32` (`0x5afc17`).
    #[test]
    fn cmsg_request_account_data_body_golden() {
        assert_eq!(request_account_data(7), vec![0x07, 0x00, 0x00, 0x00]);
    }

    /// `0x5afae0`: the section and the size as dwords, then a zlib stream at level 1, whose
    /// header is `78 01`, zlib's for that level. The deflate bytes are miniz_oxide's at level 1,
    /// a valid stream whose match choices need not be zlib 1.1.3's.
    #[test]
    fn cmsg_update_account_data_body_golden() {
        let body = update_account_data(2, b"bind W MOVEFORWARD\r\nbind S MOVEBACKWARD\r\n");
        assert_eq!(
            body,
            vec![
                0x02, 0x00, 0x00, 0x00, 0x29, 0x00, 0x00, 0x00, // section 2, 41 bytes
                0x78, 0x01, 0x4b, 0xca, 0xcc, 0x4b, 0x51, 0x08, 0x57, 0xf0, 0xf5, 0x0f, 0x73, 0x75,
                0xf3, 0x0f, 0x0a, 0x77, 0x0c, 0x72, 0xe1, 0xe5, 0x02, 0x8b, 0x05, 0x83, 0xc5, 0x9c,
                0x1c, 0x9d, 0xbd, 0x21, 0x82, 0x00, 0xfd, 0x4d, 0x0b, 0x55,
            ],
        );
    }

    #[test]
    fn an_empty_file_uploads_its_section_and_a_zero_size_alone() {
        assert_eq!(update_account_data(4, b""), vec![0x04, 0, 0, 0, 0, 0, 0, 0]);
    }

    /// `0x5afc60` reads the section and the size as dwords and inflates the rest.
    #[test]
    fn smsg_update_account_data_body_golden() {
        let file = b"MACRO 1 \"Hi\" Spell_Holy_Heal\r\n/say hi\r\nEND\r\n";
        let upload = update_account_data(4, file);
        // The server's answer has the upload's shape (`MiscHandler.cpp:877-881`).
        let parsed = read_update_account_data(&mut upload.as_slice()).unwrap();
        assert_eq!(parsed.section, 4);
        assert_eq!(parsed.size, file.len() as u32);
        assert_eq!(parsed.stream, upload[8..]);
        assert_eq!(parsed.contents().as_deref(), Some(&file[..]));
        let packet = crate::messages::parse_server(
            crate::messages::opcode::SMSG_UPDATE_ACCOUNT_DATA,
            &upload,
        );
        assert!(matches!(
            packet,
            Ok(crate::messages::ServerPacket::UpdateAccountData(u)) if u == parsed
        ));
    }

    #[test]
    fn a_zero_size_or_a_stream_that_overruns_the_size_is_no_file() {
        let empty = read_update_account_data(&mut [5u8, 0, 0, 0, 0, 0, 0, 0].as_slice()).unwrap();
        assert_eq!((empty.section, empty.size), (5, 0));
        assert!(empty.stream.is_empty());
        assert_eq!(empty.contents(), None, "size 0 writes nothing (0x5afc87)");

        let mut short =
            read_update_account_data(&mut update_account_data(6, b"Frame: A\n").as_slice())
                .unwrap();
        short.size -= 1;
        assert_eq!(
            short.contents(),
            None,
            "the stream does not fit the server's size"
        );

        let huge = UpdateAccountData {
            size: 0x1_0000,
            ..read_update_account_data(&mut update_account_data(6, b"Frame: A\n").as_slice())
                .unwrap()
        };
        assert_eq!(
            huge.contents(),
            None,
            "past 0xFFFF, the most the server stores (`MiscHandler.cpp:820`), nothing is allocated"
        );

        let junk = UpdateAccountData {
            section: 6,
            size: 9,
            stream: vec![0xde, 0xad, 0xbe, 0xef],
        };
        assert_eq!(junk.contents(), None);
    }
}
