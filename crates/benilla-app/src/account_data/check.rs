//! The section check (`0x5afa10`): one section's local, cached and server digests decide whether
//! it is uploaded, asked for, or left, and what the cache holds after.

use benilla_protocol::messages::AccountDataDigest;

/// What the check sends for a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transfer {
    /// The digests agree, or there is nothing on either side.
    Nothing,
    /// `CMSG_UPDATE_ACCOUNT_DATA` with the local file (`0x5afa8a`).
    Upload,
    /// `CMSG_REQUEST_ACCOUNT_DATA`, which leaves the section pending (`0x5afa6c`).
    Request,
}

/// The check's verdict: the send, and the digest the cache takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    pub(crate) send: Transfer,
    pub(crate) cache: AccountDataDigest,
}

/// `0x5afa10(section, local, cache, server)`. With a zero server digest the local file goes up
/// unless it too is zero (`0x5afa26`, `0x5afa49`, `0x5afa79`). Otherwise a local file changed since
/// the last agreement (`local != cache`, `0x5afa54`) goes up when the server's differs, and an
/// unchanged one is asked for when the server's differs (`0x5afa65`). The cache becomes the local
/// digest, except for an unchanged file, where it becomes the server's (`0x5afa74`, `0x5afa92`).
pub(crate) fn check(
    local: &AccountDataDigest,
    cache: &AccountDataDigest,
    server: &AccountDataDigest,
) -> Checked {
    let server_has_none = server.iter().all(|&b| b == 0);
    if server_has_none || local != cache {
        let send = if local != server {
            Transfer::Upload
        } else {
            Transfer::Nothing
        };
        return Checked {
            send,
            cache: *local,
        };
    }
    let send = if local != server {
        Transfer::Request
    } else {
        Transfer::Nothing
    };
    Checked {
        send,
        cache: *server,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO: AccountDataDigest = [0; 16];
    const A: AccountDataDigest = [0xaa; 16];
    const B: AccountDataDigest = [0xbb; 16];
    const C: AccountDataDigest = [0xcc; 16];

    fn checked(send: Transfer, cache: AccountDataDigest) -> Checked {
        Checked { send, cache }
    }

    /// Every branch of `0x5afa10`, by (local, cache, server).
    #[test]
    fn the_check_uploads_a_changed_file_asks_for_a_changed_server_and_caches_what_agrees() {
        // The server has none: a local file goes up, cached as itself.
        assert_eq!(check(&A, &ZERO, &ZERO), checked(Transfer::Upload, A));
        assert_eq!(check(&A, &A, &ZERO), checked(Transfer::Upload, A));
        assert_eq!(check(&ZERO, &B, &ZERO), checked(Transfer::Nothing, ZERO));
        assert_eq!(check(&ZERO, &ZERO, &ZERO), checked(Transfer::Nothing, ZERO));
        // Changed locally since the last agreement: local wins over a different server.
        assert_eq!(check(&A, &B, &C), checked(Transfer::Upload, A));
        assert_eq!(check(&A, &B, &A), checked(Transfer::Nothing, A));
        assert_eq!(check(&ZERO, &B, &C), checked(Transfer::Upload, ZERO));
        // Unchanged locally: a different server copy is asked for and cached as the server's.
        assert_eq!(check(&A, &A, &C), checked(Transfer::Request, C));
        assert_eq!(check(&A, &A, &A), checked(Transfer::Nothing, A));
        assert_eq!(
            check(&ZERO, &ZERO, &C),
            checked(Transfer::Request, C),
            "no file here and none before: the server's copy comes down"
        );
    }
}
