//! Action-bar messages: the 120-slot login snapshot, the one-slot write, and the bar toggles.
//! The bar is client-authoritative: the server stores the slots, sends them at login, and takes
//! one `CMSG_SET_ACTION_BUTTON` per local change.

use std::io::{self};

use crate::wire::read_u32_le;

/// The action-button kind, bits 24-31 of a slot word (`ActionButtonType`, vmangos `Player.h`).
pub const ACTION_KIND_SPELL: u8 = 0x00;
pub const ACTION_KIND_MACRO: u8 = 0x40;
pub const ACTION_KIND_ITEM: u8 = 0x80;

/// An occupied slot of `SMSG_ACTION_BUTTONS`, 120 packed `u32`s
/// (`MasterPlayer::SendInitialActionButtons`); a zero word is an empty slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionButton {
    /// The bar slot index (0..119). Slots 0–11 are the main bar's buttons 1–12.
    pub slot: u8,
    /// The spell/macro/item id (bits 0–23). A character macro's id, which has bit 24 set
    /// (`benilla_ui::script::macro_slot_id`), keeps it in `kind`: 0x41, vmangos `ACTION_BUTTON_CMACRO`.
    pub action: u32,
    /// The kind byte (bits 24-31), an `ACTION_KIND_*`; the enum's 0x01 ("click?") is carried raw.
    pub kind: u8,
}

/// Reads slot words to the end of the body (the server sends 120), dropping empty ones.
pub(super) fn read_action_buttons(r: &mut &[u8]) -> io::Result<Vec<ActionButton>> {
    let mut buttons = Vec::new();
    let mut slot: u32 = 0;
    while !r.is_empty() {
        let packed = read_u32_le(r)?;
        if packed != 0 {
            buttons.push(ActionButton {
                slot: slot.min(u8::MAX as u32) as u8,
                action: packed & 0x00FF_FFFF,
                kind: (packed >> 24) as u8,
            });
        }
        slot += 1;
    }
    Ok(buttons)
}

/// `CMSG_SET_ACTION_BUTTON` (296): `button u8`, `packetData u32` packed as in [`ActionButton`]
/// (`Server/Packets/Misc.cpp:87-90`). Zero clears the slot (no reply); a drag-swap is two sends.
pub fn set_action_button(button: u8, packed: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(5);
    body.push(button);
    body.extend_from_slice(&packed.to_le_bytes());
    body
}

/// `CMSG_SET_ACTIONBAR_TOGGLES`: one `u8`, the whole body (`SetActionBarToggles 0x4e76e0`).
/// The byte is server-owned: the reference never writes its copy (`PLAYER_FIELD_BYTES` byte 2),
/// which changes only when `SMSG_UPDATE_OBJECT` echoes it, with no change event; read it with
/// [`super::ObjectFields::player_action_bar_toggles`]. The reference packs only bits 0..3, so a
/// set clears the high nibble; this encoder does not mask. Bits name bars only in FrameXML.
pub fn set_actionbar_toggles(toggles: u8) -> Vec<u8> {
    vec![toggles]
}
