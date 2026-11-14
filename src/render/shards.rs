//! Pass 2 quads for the shards of burst windows and the bullet holes on
//! windows still standing.

use glam::{Mat4, Vec3};

use super::scene::Quad;
use crate::{
    shooting::CorpseEntry,
    world::{Camera, Panel, combat::Damage},
};

/// The colour a freshly burst window glows.
const BURST_GLOW: Vec3 = Vec3::new(1.0, 0.9, 0.7);

/// One quad per visible shard, each with its distance from the camera, squared,
/// for sorting.
pub fn quads(corpses: &[CorpseEntry], camera: &Camera) -> Vec<(f32, Quad)> {
    let mut quads = Vec::new();
    for entry in corpses {
        let glow = (BURST_GLOW * entry.corpse.flash()).to_array();
        for shard in &entry.corpse.shards {
            let alpha = shard.alpha();
            if alpha <= 0.0 {
                continue;
            }
            let size = shard.uv_max - shard.uv_min;
            quads.push((
                shard.pos.distance_squared(camera.position),
                Quad {
                    uv_rect: [shard.uv_min.x, shard.uv_min.y, size.x, size.y],
                    tint: [alpha; 4],
                    flash: glow,
                    ..Quad::new(shard.model(), entry.texture.id())
                },
            ));
        }
    }
    quads
}

/// Width of a bullet hole's picture, in metres.
const HOLE_SIZE: f32 = 0.12;
/// How far in front of the window's surface holes are drawn, in metres, so
/// that they win the depth test.
const HOLE_LIFT: f32 = 0.002;

/// One quad for each of `damage`'s bullet holes, stuck on `panel` (the
/// window's picture, as it is displayed) and drawn with `texture`.
pub fn holes(panel: &Panel, damage: &Damage, texture: u32) -> Vec<Quad> {
    damage
        .holes
        .iter()
        .map(|hole| {
            let local = Vec3::new(
                (hole.at.x - 0.5) * panel.size.x,
                (0.5 - hole.at.y) * panel.size.y,
                HOLE_LIFT,
            );
            let model = Mat4::from_scale_rotation_translation(
                Vec3::new(HOLE_SIZE, HOLE_SIZE, 1.0),
                panel.rotation,
                panel.center + panel.rotation * local,
            );
            Quad {
                tint: [hole.alpha(); 4],
                ..Quad::new(model, texture)
            }
        })
        .collect()
}
