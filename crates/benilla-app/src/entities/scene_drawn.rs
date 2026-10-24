//! Whether the scene drew a unit in the last render: the reference's scene-node test `0x6704c0`
//! (`node && [node+0x94] == 0`), which the chat bubble (`0x4b0cab`) and the plate gate
//! (`0x60f649`) both read off the unit's scene node `[unit+0xe0]`.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// A unit's meshes and their last-frame visibility. Every frame sets `[node+0x94]` on every node
/// in the scene list `[0xca7d70]` (`0x698a07`) and clears it only for a node that passes the
/// frustum test `0x682ef0` and the horizon test `0x686000` (`0x6833dd`-`0x6833ec`, and
/// `0x68359b`-`0x6835aa` in WMO groups). Here that is any of the unit's meshes passing last
/// frame's visibility check, the frustum and the indoor portal walk; benilla has no horizon cull
/// yet, and a unit with no model yet has no node.
#[derive(SystemParam)]
pub(crate) struct SceneDrawn<'w, 's> {
    children: Query<'w, 's, &'static Children>,
    meshes: Query<'w, 's, &'static ViewVisibility, With<Mesh3d>>,
}

impl SceneDrawn<'_, '_> {
    /// Whether the last render drew any of `unit`'s meshes.
    pub(crate) fn unit(&self, unit: Entity) -> bool {
        self.children.get(unit).is_ok_and(|kids| {
            kids.iter()
                .any(|c| self.meshes.get(c).is_ok_and(|v| v.get()))
        })
    }
}
