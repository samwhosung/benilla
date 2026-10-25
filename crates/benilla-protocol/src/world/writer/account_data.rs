//! The account-data sends: a section asked for, and a section stored.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_REQUEST_ACCOUNT_DATA`: ask for `section`'s file, whose digest differs from the
    /// server's while the local file is unchanged since they last agreed.
    pub fn request_account_data(&mut self, section: u32) -> Result<()> {
        self.send(
            opcode::CMSG_REQUEST_ACCOUNT_DATA,
            &messages::request_account_data(section),
        )
    }

    /// `CMSG_UPDATE_ACCOUNT_DATA`: store `contents` as `section`'s file on the server.
    pub fn update_account_data(&mut self, section: u32, contents: &[u8]) -> Result<()> {
        self.send(
            opcode::CMSG_UPDATE_ACCOUNT_DATA,
            &messages::update_account_data(section, contents),
        )
    }
}
