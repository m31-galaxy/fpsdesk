//! Closing applications by shooting them.
//!
//! Every hit on the window under the crosshair flashes it red, knocks it
//! about and leaves a bullet hole. The holes are its health: a hit counts
//! until its hole has faded, and the hit that finds two holes already there
//! is the killing shot (`world::combat::Damage`). Only then is the window
//! asked to close, politely: the client may put up a dialog or ignore the
//! request, and force-closing would lose the user's work. If it really goes
//! away, its last picture and place stay behind as a corpse that bursts into
//! shards. A window that closes for any other reason just vanishes, and one
//! still there `SURVIVAL_TIME` after the killing shot recovers.

use glam::Vec2;
use smithay::utils::IsAlive;
use tracing::info;

use crate::{
    input::controls::Mode,
    render::{weapon::WeaponView, window::WindowTexture},
    state::State,
    world::combat::{Combat, Corpse},
};

/// A killed window's remains: the shards, and the picture they show.
pub struct CorpseEntry {
    pub corpse: Corpse,
    pub texture: WindowTexture,
}

#[derive(Default)]
pub struct Shooting {
    pub combat: Combat,
    pub corpses: Vec<CorpseEntry>,
    /// Gives each burst its own pattern.
    bursts: u32,
}

impl State {
    /// Fire at whatever the crosshair is on. Walk mode only.
    pub fn shoot(&mut self) {
        self.shooting.combat.fire();

        let Some(window) = self.hovered.clone() else {
            return;
        };
        let ray = self.ray_through(Vec2::ZERO);
        let Some(entry) = self.windows.iter_mut().find(|entry| entry.window == window) else {
            return;
        };

        let impact = ray
            .and_then(|ray| entry.content_panel()?.intersect_plane(&ray))
            .map_or(Vec2::splat(0.5), |hit| hit.uv.clamp(Vec2::ZERO, Vec2::ONE));
        entry.damage.last_panel = entry.content_panel();
        let lethal = entry.damage.hit(impact);
        if lethal && let Some(toplevel) = entry.window.toplevel() {
            toplevel.send_close();
        }
        info!(
            slot = entry.slot,
            holes = entry.damage.holes.len(),
            lethal,
            ?impact,
            "shot a window"
        );
    }

    /// Advance the gun, the shot windows and the corpses by `dt` seconds.
    pub fn step_shooting(&mut self, dt: f32) {
        // A kill freezes the effects but not the viewer.
        let effects_dt = self.shooting.combat.effects_dt(dt);
        let pitch = self.shooting.combat.gun.step(effects_dt);
        self.camera.turn(0.0, pitch);

        let corpses = &mut self.shooting.corpses;
        for entry in corpses.iter_mut() {
            entry.corpse.step(effects_dt);
        }
        corpses.retain(|entry| !entry.corpse.is_done());

        for entry in &mut self.windows {
            entry.damage.step(dt);
            // Remember where the picture is, in case this is its last frame.
            if entry.damage.kill_impact().is_some()
                && let Some(panel) = entry.content_panel()
            {
                entry.damage.last_panel = Some(panel);
            }
        }
    }

    /// Forget windows whose clients have gone. Those that were shot dead
    /// leave a corpse; the rest simply vanish.
    pub fn bury_dead_windows(&mut self) {
        let mut index = 0;
        while index < self.windows.len() {
            if self.windows[index].window.alive() {
                index += 1;
                continue;
            }
            let mut entry = self.windows.remove(index);
            // The window's extent is forgotten once its surface is gone, so
            // the place it was comes from the last frame it was seen in.
            let (Some(impact), Some(content), Some(texture)) = (
                entry.damage.kill_impact(),
                entry.damage.last_panel,
                entry.texture.take(),
            ) else {
                continue;
            };

            let shooting = &mut self.shooting;
            shooting.bursts = shooting.bursts.wrapping_add(1);
            // Burst from where the window was when it died, mid-wobble.
            let panel = entry
                .damage
                .pose()
                .map_or(content, |pose| pose.apply(&content));
            let corpse = Corpse::burst(&panel, impact, shooting.bursts);
            shooting.corpses.push(CorpseEntry { corpse, texture });
            shooting.combat.kill();
            info!(slot = entry.slot, "shot window died");
        }
    }

    /// What to draw of the weapon, if it should be on screen.
    pub fn weapon_view(&self) -> Option<WeaponView> {
        (self.mode == Mode::Walk).then(|| WeaponView {
            kick: self.shooting.combat.gun.kick(),
            flash: self.shooting.combat.gun.flash(),
        })
    }
}
