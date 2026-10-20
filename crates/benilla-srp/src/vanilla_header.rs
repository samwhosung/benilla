//! The 1.12 world-packet header obfuscation. After `CMSG_AUTH_SESSION` every header, not the body,
//! runs through a byte cipher keyed by the 40-byte session key, `i` cycling over the key and
//! `last_c` the previous ciphertext byte, both starting at 0:
//!
//! ```text
//! encrypt: c = (p ^ key[i]) + last_c ;  decrypt: p = (c - last_c) ^ key[i]
//! ```
//!
//! A header is the size, big-endian in two bytes, or in three above `0x7fff` with the first's top
//! bit set (`0x5b56da`, `0x5b674d`), then the opcode: four bytes little-endian from the client, two
//! from the server. The framing is `benilla-protocol`'s; the cipher runs over whichever header it
//! writes or reads. vmangos writes the two-byte form alone, truncating a larger size
//! (`WorldSocket.cpp:602`).

use rand::{thread_rng, RngCore};
use sha1::{Digest, Sha1};

use crate::{NormalizedString, SESSION_KEY_LENGTH};

/// `world_server_proof = SHA1( user | 0u32 | client_seed | server_seed | K )`.
fn world_server_proof(
    username: &NormalizedString,
    session_key: &[u8; SESSION_KEY_LENGTH],
    server_seed: u32,
    client_seed: u32,
) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(username.as_ref().as_bytes());
    h.update(0u32.to_le_bytes());
    h.update(client_seed.to_le_bytes());
    h.update(server_seed.to_le_bytes());
    h.update(session_key);
    h.finalize().into()
}

/// The header cipher's encrypting half, for the write side of a split connection.
#[derive(Debug, Clone)]
pub struct EncrypterHalf {
    session_key: [u8; SESSION_KEY_LENGTH],
    index: u8,
    previous: u8,
}

impl EncrypterHalf {
    /// Encrypt `data` in place, advancing the cipher state.
    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data {
            let encrypted =
                (*byte ^ self.session_key[self.index as usize]).wrapping_add(self.previous);
            self.index = (self.index + 1) % SESSION_KEY_LENGTH as u8;
            *byte = encrypted;
            self.previous = encrypted;
        }
    }
}

/// The header cipher's decrypting half, for the read side of a split connection.
#[derive(Debug, Clone)]
pub struct DecrypterHalf {
    session_key: [u8; SESSION_KEY_LENGTH],
    index: u8,
    previous: u8,
}

impl DecrypterHalf {
    /// Decrypt `data` in place, advancing the cipher state.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        for byte in data {
            let encrypted = *byte;
            let decrypted =
                encrypted.wrapping_sub(self.previous) ^ self.session_key[self.index as usize];
            self.index = (self.index + 1) % SESSION_KEY_LENGTH as u8;
            self.previous = encrypted;
            *byte = decrypted;
        }
    }
}

/// Both halves of the header cipher.
#[derive(Debug, Clone)]
pub struct HeaderCrypto {
    encrypt: EncrypterHalf,
    decrypt: DecrypterHalf,
}

impl HeaderCrypto {
    pub fn decrypter(&mut self) -> &mut DecrypterHalf {
        &mut self.decrypt
    }

    pub fn encrypter(&mut self) -> &mut EncrypterHalf {
        &mut self.encrypt
    }

    /// Split into halves, one per direction of a split socket.
    pub fn split(self) -> (EncrypterHalf, DecrypterHalf) {
        (self.encrypt, self.decrypt)
    }

    /// The halves [`Self::split`] made, joined again with the state each direction reached.
    pub fn join(encrypt: EncrypterHalf, decrypt: DecrypterHalf) -> Self {
        Self { encrypt, decrypt }
    }

    /// The cipher from a session key alone, skipping the proof: a server's [`EncrypterHalf`] is
    /// what a client's [`DecrypterHalf`] inverts, so a fake world server speaks real headers.
    pub fn from_session_key(session_key: [u8; SESSION_KEY_LENGTH]) -> Self {
        Self::new(session_key)
    }

    fn new(session_key: [u8; SESSION_KEY_LENGTH]) -> Self {
        Self {
            encrypt: EncrypterHalf {
                session_key,
                index: 0,
                previous: 0,
            },
            decrypt: DecrypterHalf {
                session_key,
                index: 0,
                previous: 0,
            },
        }
    }
}

/// The client's random seed for the world handshake.
#[derive(Debug, Clone, Copy)]
pub struct ProofSeed {
    seed: u32,
}

impl ProofSeed {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            seed: thread_rng().next_u32(),
        }
    }

    /// The client seed to send in `CMSG_AUTH_SESSION`.
    pub const fn seed(&self) -> u32 {
        self.seed
    }

    /// The client proof, binding the session key to both seeds, and the header cipher.
    pub fn into_client_header_crypto(
        self,
        username: &NormalizedString,
        session_key: [u8; SESSION_KEY_LENGTH],
        server_seed: u32,
    ) -> ([u8; 20], HeaderCrypto) {
        let proof = world_server_proof(username, &session_key, server_seed, self.seed);
        (proof, HeaderCrypto::new(session_key))
    }
}
