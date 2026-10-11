//! The chat bubble widgets: the reference's `CGChatBubbleFrame`.

use super::UiScript;

/// Region 1's art, the backdrop's background, and the path bubble addons identify a bubble by.
pub const BG_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Background";
/// Regions 2 to 9, the backdrop's eight border pieces.
pub const EDGE_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Backdrop";
/// Region 10, the tail.
pub const TAIL_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Tail";

/// The bubble geometry that follows from the window, in the `WorldFrame`'s units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BubbleGeometry {
    pub border: f32,
    pub margin: f32,
    pub wrap_cap: f32,
    pub text_height: f32,
    pub device_px: f32,
}

/// One live bubble's state for this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct BubbleState {
    pub key: u64,
    pub line: u64,
    pub text: String,
    pub colour: [f32; 3],
    pub shown: bool,
    pub alpha: f32,
    pub seat: (f32, f32),
}

impl UiScript {
    /// Drive the bubble pool from the app's live bubbles.
    pub fn sync_chat_bubbles(&mut self, geometry: BubbleGeometry, states: &[BubbleState]) {
        let _ = (geometry, states);
    }

    /// How many bubble widgets the pool holds.
    pub fn chat_bubble_pool_len(&self) -> usize {
        0
    }
}
