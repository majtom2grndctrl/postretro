// Door fixtures for shot, splash and sight tests across crates.
// See: context/lib/testing_guide.md §4 (Test fixtures)

use std::collections::HashMap;

use glam::{Quat, Vec3};
use postretro_entities::Transform;

use super::moving::{MoverCollider, MoverPose, MoverPoseSource};

/// A closed box door, local to its origin: `half_thickness` along X, `height`
/// up from the origin, `half_width` along Z.
pub fn door_slab_triangles(
    half_thickness: f32,
    height: f32,
    half_width: f32,
) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (x, y1, z) = (half_thickness, height, half_width);
    let vertices = vec![
        Vec3::new(-x, 0.0, -z),
        Vec3::new(x, 0.0, -z),
        Vec3::new(x, y1, -z),
        Vec3::new(-x, y1, -z),
        Vec3::new(-x, 0.0, z),
        Vec3::new(x, 0.0, z),
        Vec3::new(x, y1, z),
        Vec3::new(-x, y1, z),
    ];
    let triangles = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 6, 2],
        [3, 7, 6],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ];
    (vertices, triangles)
}

/// [`door_slab_triangles`] as a mover collider.
pub fn door_slab(
    mover_id: u32,
    half_thickness: f32,
    height: f32,
    half_width: f32,
) -> MoverCollider {
    let (vertices, triangles) = door_slab_triangles(half_thickness, height, half_width);
    MoverCollider::from_local_triangles(mover_id, &vertices, &triangles)
        .expect("door slab is a valid mesh")
}

/// A pose source holding one still transform per mover.
#[derive(Debug, Default, Clone)]
pub struct DoorPoses(pub HashMap<u32, Transform>);

impl DoorPoses {
    pub fn place(mut self, mover_id: u32, position: Vec3, rotation: Quat) -> Self {
        self.set(mover_id, position, rotation);
        self
    }

    pub fn set(&mut self, mover_id: u32, position: Vec3, rotation: Quat) {
        self.0.insert(
            mover_id,
            Transform {
                position,
                rotation,
                scale: Vec3::ONE,
            },
        );
    }
}

impl MoverPoseSource for DoorPoses {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.0.get(&mover_id).map(|transform| MoverPose {
            transform: *transform,
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            tick_rotation_delta: Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: 1.0 / 60.0,
        })
    }
}
