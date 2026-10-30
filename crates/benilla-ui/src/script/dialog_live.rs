//! What the dialogs' getters ask the lent host as they run ([`super::live::ask`]), each as of the
//! place the getter runs. The reference's getters read the dialog globals at the call: the three
//! service latches (`0xc4d7c0`, `0xc4d7b0`, `0x48d210`, `0x48d1c0`), the boot and spirit-healer
//! deadlines (`0xb4e34c`, `0xb4e338`), the meeting stone's area and text (`0xb72038`, `0xb7203c`),
//! the summon latch (`0xb4e350`-`0xb4e35c`) and the instance terms (`0x48a750`, `0x495c90`), which
//! the packet handlers write inside themselves. A listener fired after its packet's handlers reads
//! them as its packet left them. With no host to ask, the VM's copy answers; only a test sets it,
//! as every production entry that runs Lua lends the host.

use mlua::Lua;

use super::live::LiveQuery;
use super::SummonConfirmUiState;

/// `CheckBinderDist()`'s latch: a bind question is held for an innkeeper in reach (`0x5dff50`).
pub struct BinderPendingQuery;

impl LiveQuery for BinderPendingQuery {
    type Answer = bool;
}

/// `CheckPetUntrainerDist()`'s latch: an unlearn question is held for a trainer in reach
/// (`0x5dfd30`).
pub struct PetUntrainerPendingQuery;

impl LiveQuery for PetUntrainerPendingQuery {
    type Answer = bool;
}

/// `GetInstanceBootTimeRemaining()`'s whole seconds at the call (`0x496380`, `0x48b630`).
pub struct InstanceBootSecsQuery;

impl LiveQuery for InstanceBootSecsQuery {
    type Answer = u32;
}

/// `GetAreaSpiritHealerTime()`'s whole seconds at the call (`[0xb4e338]`).
pub struct AreaSpiritHealerSecsQuery;

impl LiveQuery for AreaSpiritHealerSecsQuery {
    type Answer = u32;
}

/// What the status text global `0xb7203c` holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MeetingStoneText {
    /// Null: before the world entry and after the world leave (`0x4c9f80`).
    #[default]
    Unset,
    /// The enter cascade's copy of the global `UNKNOWN` (`0x4c9f40`).
    Unknown,
    /// The `0x295` rebuild (`0x4ca070`) over the queued area, by its name, `None` for an id
    /// without a row, which names it `UNKNOWN`.
    Built(Option<String>),
}

/// The meeting stone's two globals at the call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeetingStoneStatus {
    /// `[0xb72038]`, 0 when not queued.
    pub area: u32,
    pub text: MeetingStoneText,
}

/// The question: [`MeetingStoneStatus`] now.
pub struct MeetingStoneQuery;

impl LiveQuery for MeetingStoneQuery {
    type Answer = MeetingStoneStatus;
}

/// `SStrPrintf`'s buffer at the rebuild: 256 bytes, so 255 of text.
const STONE_TEXT_BYTES: usize = 255;

/// The status text `text` reads as, against the VM's strings: `0x4ca070`'s rebuild formats
/// `MEETINGSTONE_TOOLTIP` (`""` when missing, `0x703bf0`) over the area's name or `UNKNOWN`, into
/// 256 bytes; `None` while unset.
pub fn meeting_stone_text(lua: &Lua, text: &MeetingStoneText) -> Option<String> {
    let global = |key: &str| lua.globals().get::<String>(key).unwrap_or_default();
    match text {
        MeetingStoneText::Unset => None,
        MeetingStoneText::Unknown => Some(global("UNKNOWN")),
        MeetingStoneText::Built(name) => {
            let name = name.clone().unwrap_or_else(|| global("UNKNOWN"));
            let mut text = global("MEETINGSTONE_TOOLTIP").replacen("%s", &name, 1);
            if text.len() > STONE_TEXT_BYTES {
                let mut cut = STONE_TEXT_BYTES;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
            }
            Some(text)
        }
    }
}

/// The summon dialog's three reads at the call ([`SummonConfirmUiState`]): the latched summoner's
/// cached name, the latched zone's row name and the deadline less now (`0x48b6a0`, `0x48b720`,
/// `0x4963e0`).
pub struct SummonConfirmQuery;

impl LiveQuery for SummonConfirmQuery {
    type Answer = SummonConfirmUiState;
}

/// `IsInInstance()`'s `Map.dbc` `InstanceType` of the map the world stands on (`0x48a750`), `None`
/// for a map with no row.
pub struct InstanceTypeQuery;

impl LiveQuery for InstanceTypeQuery {
    type Answer = Option<u32>;
}

/// `CanShowResetInstances()`'s four terms at the call (`0x495c90`).
pub struct CanResetInstancesQuery;

impl LiveQuery for CanResetInstancesQuery {
    type Answer = bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rebuild's three shapes, and the 255-byte cut at a character boundary.
    #[test]
    fn the_status_text_formats_the_tooltip_over_the_area_or_unknown() {
        let s = crate::script::UiScript::new().unwrap();
        let lua = s.lua();
        assert_eq!(meeting_stone_text(lua, &MeetingStoneText::Unset), None);
        assert_eq!(
            meeting_stone_text(lua, &MeetingStoneText::Built(Some("Elwynn".into()))).as_deref(),
            Some(""),
            "a missing template is the empty string"
        );
        s.run(r#"UNKNOWN = "Unknown" MEETINGSTONE_TOOLTIP = "Looking for more for %s""#)
            .unwrap();
        assert_eq!(
            meeting_stone_text(lua, &MeetingStoneText::Unknown).as_deref(),
            Some("Unknown")
        );
        assert_eq!(
            meeting_stone_text(lua, &MeetingStoneText::Built(None)).as_deref(),
            Some("Looking for more for Unknown")
        );
        let long = "é".repeat(200);
        let text = meeting_stone_text(lua, &MeetingStoneText::Built(Some(long))).unwrap();
        assert!(text.len() <= STONE_TEXT_BYTES && text.len() >= STONE_TEXT_BYTES - 1);
    }
}
