//! Shooting: the gun's recoil, a window's reaction to being hit, and the
//! shards a killed window bursts into.
//!
//! Pure maths driven by `dt`, like the rest of `world/`.

use glam::{Mat4, Quat, Vec2, Vec3};

use super::Panel;

/// Hits a window can take while the holes are still showing: the next one
/// kills it.
pub const HITS_TO_KILL: usize = 3;
/// A bullet hole stays fully visible this long, in seconds, then fades out
/// over `HOLE_FADE` more.
pub const HOLE_HOLD: f32 = 10.0;
pub const HOLE_FADE: f32 = 8.0;
/// A window still there this long after the killing shot has survived it.
pub const SURVIVAL_TIME: f32 = 3.0;
/// How long a hit's knock and flash are tracked, in seconds.
const REACTION_TIME: f32 = 2.0;

/// A kill freezes the effects for this long, in seconds.
const HIT_STOP: f32 = 0.07;

const GRAVITY: f32 = 9.8;
/// Shards fade out over the last part of their life, in seconds.
const FADE_TIME: f32 = 0.7;
/// How long a freshly burst window glows white, in seconds.
const BURST_FLASH_TIME: f32 = 0.15;
const SHARD_COLUMNS: usize = 8;

/// A small deterministic random number generator (xorshift), so that bursts
/// are reproducible and the simulation needs no dependency.
struct Rng(u32);

impl Rng {
    /// Uniform in 0 to 1.
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in -1 to 1.
    fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

/// A damped spring that kicks the view up and lets it settle.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Recoil {
    /// Pitch above rest, in radians.
    offset: f32,
    velocity: f32,
}

impl Recoil {
    const OMEGA: f32 = 14.0;
    const DAMPING: f32 = 0.7;
    /// Initial upward speed of a shot, in radians a second.
    const KICK: f32 = 1.4;

    /// Advance by `dt`; returns how much the offset changed.
    fn step(&mut self, dt: f32) -> f32 {
        let before = self.offset;
        // Fixed small steps keep the spring stable after a long frame.
        let steps = (dt * 240.0).ceil().max(1.0);
        let h = dt / steps;
        for _ in 0..steps as u32 {
            let accel = -Self::OMEGA * Self::OMEGA * self.offset
                - 2.0 * Self::DAMPING * Self::OMEGA * self.velocity;
            self.velocity += accel * h;
            self.offset += self.velocity * h;
        }
        self.offset - before
    }
}

/// The weapon: what shows when it fires.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Gun {
    recoil: Recoil,
    flash: f32,
    kick: f32,
}

impl Gun {
    const FLASH_TIME: f32 = 0.09;
    const KICK_TIME: f32 = 0.22;

    pub fn fire(&mut self) {
        self.flash = 1.0;
        self.kick = 1.0;
        self.recoil.velocity += Recoil::KICK;
    }

    /// Advance by `dt`. Returns the change to make to the camera's pitch.
    pub fn step(&mut self, dt: f32) -> f32 {
        self.flash = (self.flash - dt / Self::FLASH_TIME).max(0.0);
        self.kick = (self.kick - dt / Self::KICK_TIME).max(0.0);
        self.recoil.step(dt)
    }

    /// Muzzle flash brightness: 1 as the shot goes off, 0 a moment later.
    pub fn flash(&self) -> f32 {
        self.flash
    }

    /// How far the weapon is thrown back on screen: 1 to 0.
    pub fn kick(&self) -> f32 {
        self.kick
    }

    /// Pitch the view is currently kicked above rest, in radians.
    #[cfg(test)]
    fn pitch_offset(&self) -> f32 {
        self.recoil.offset
    }
}

/// The gun and the clock for effects, which a kill can freeze for a moment.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Combat {
    pub gun: Gun,
    hit_stop: f32,
}

impl Combat {
    pub fn fire(&mut self) {
        self.gun.fire();
    }

    /// A window was killed: hold the effects still for a few frames.
    pub fn kill(&mut self) {
        self.hit_stop = HIT_STOP;
    }

    /// How much time the effects get out of `dt` real seconds: none while
    /// frozen.
    pub fn effects_dt(&mut self, dt: f32) -> f32 {
        if self.hit_stop > 0.0 {
            self.hit_stop -= dt;
            0.0
        } else {
            dt
        }
    }
}

/// A bullet hole on a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hole {
    /// Where it is, as fractions of the window's picture from its top-left
    /// corner.
    pub at: Vec2,
    /// Seconds since the shot.
    pub age: f32,
}

impl Hole {
    /// 1 while fresh, then fading to 0 (healed).
    pub fn alpha(&self) -> f32 {
        (1.0 - (self.age - HOLE_HOLD) / HOLE_FADE).clamp(0.0, 1.0)
    }
}

/// A window's injuries. Its health is its visible holes: a hit counts for as
/// long as its hole shows, and the hit that finds `HITS_TO_KILL - 1` holes
/// already there is lethal.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Damage {
    pub holes: Vec<Hole>,
    /// The latest hit, while its knock and flash last.
    reaction: Option<Shot>,
    /// The killing shot, while it is not known whether the window will go.
    kill: Option<Shot>,
    /// The window's picture's place in the world when last seen, for the
    /// corpse to appear where it was.
    pub last_panel: Option<Panel>,
}

impl Damage {
    /// Take a hit at `impact`. Returns whether it was lethal, in which case
    /// the window should be asked to close.
    pub fn hit(&mut self, impact: Vec2) -> bool {
        self.holes.push(Hole {
            at: impact,
            age: 0.0,
        });
        self.reaction = Some(Shot::new(impact));
        let lethal = self.holes.iter().filter(|hole| hole.alpha() > 0.0).count() >= HITS_TO_KILL;
        if lethal {
            self.kill = Some(Shot::new(impact));
        }
        lethal
    }

    pub fn step(&mut self, dt: f32) {
        for hole in &mut self.holes {
            hole.age += dt;
        }
        self.holes.retain(|hole| hole.alpha() > 0.0);

        if let Some(reaction) = &mut self.reaction {
            reaction.age += dt;
            if reaction.age > REACTION_TIME {
                self.reaction = None;
            }
        }
        if let Some(kill) = &mut self.kill {
            kill.age += dt;
            if kill.age > SURVIVAL_TIME {
                // Still here: it survived, and recovers.
                self.kill = None;
                self.holes.clear();
            }
        }
    }

    /// How the latest hit is displacing and lighting the window.
    pub fn pose(&self) -> Option<Pose> {
        self.reaction.map(|shot| shot.pose())
    }

    /// Where the killing shot landed, if one is awaiting its outcome.
    pub fn kill_impact(&self) -> Option<Vec2> {
        self.kill.map(|kill| kill.impact)
    }
}

/// The reaction of a window to a hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    /// Seconds since the hit.
    pub age: f32,
    /// Where the shot landed, as fractions of the window's picture from its
    /// top-left corner.
    pub impact: Vec2,
}

/// How a shot window is displaced and lit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// Metres pushed away from the viewer.
    pub push: f32,
    /// Rotation about the panel's vertical axis and horizontal axis, radians.
    pub yaw: f32,
    pub pitch: f32,
    /// Red flash strength, 1 to 0.
    pub flash: f32,
}

impl Shot {
    const FLASH_TIME: f32 = 0.45;

    pub fn new(impact: Vec2) -> Self {
        Self { age: 0.0, impact }
    }

    /// The window's reaction: knocked back and rocked about the point of
    /// impact, ringing down to rest within a second or so.
    pub fn pose(&self) -> Pose {
        let a = self.age;
        let ring = (-5.0 * a).exp() * (25.0 * a).cos();
        Pose {
            push: 0.12 * (-7.0 * a).exp(),
            // A hit on the right pushes the right side back, and so on.
            yaw: (self.impact.x - 0.5) * 0.5 * ring,
            pitch: (self.impact.y - 0.5) * 0.5 * ring,
            flash: (1.0 - a / Self::FLASH_TIME).max(0.0),
        }
    }
}

impl Pose {
    /// `panel` as displaced by this pose.
    pub fn apply(&self, panel: &Panel) -> Panel {
        Panel {
            center: panel.center - panel.rotation * Vec3::Z * self.push,
            rotation: panel.rotation
                * Quat::from_rotation_y(self.yaw)
                * Quat::from_rotation_x(self.pitch),
            size: panel.size,
        }
    }
}

/// One piece of a burst window: a rectangle of its picture, tumbling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shard {
    pub pos: Vec3,
    pub rot: Quat,
    /// Width and height in metres.
    pub size: Vec2,
    /// The part of the window's picture it shows, as fractions from the
    /// top-left corner.
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    pub vel: Vec3,
    /// Angular velocity, radians a second about each axis.
    pub spin: Vec3,
    pub age: f32,
    /// Seconds until it has faded away.
    pub life: f32,
}

impl Shard {
    pub fn model(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.size.extend(1.0), self.rot, self.pos)
    }

    /// 1 while solid, fading to 0 at the end of its life.
    pub fn alpha(&self) -> f32 {
        ((self.life - self.age) / FADE_TIME).clamp(0.0, 1.0)
    }

    fn radius(&self) -> f32 {
        0.35 * self.size.max_element()
    }

    pub fn step(&mut self, dt: f32) {
        self.age += dt;
        self.vel.y -= GRAVITY * dt;
        self.pos += self.vel * dt;
        self.rot = (Quat::from_scaled_axis(self.spin * dt) * self.rot).normalize();

        let radius = self.radius();
        if self.pos.y < radius {
            self.pos.y = radius;
            if self.vel.y < 0.0 {
                self.vel.y = -self.vel.y * 0.45;
                // Scrape and tumble less after each landing.
                self.vel.x *= 0.8;
                self.vel.z *= 0.8;
                self.spin *= 0.7;
            }
            if self.vel.y < 0.4 {
                self.vel.y = 0.0;
                let drag = (-6.0 * dt).exp();
                self.vel.x *= drag;
                self.vel.z *= drag;
                self.spin *= drag;
            }
        }
    }
}

/// What is left of a window that was shot dead.
#[derive(Debug, Clone, PartialEq)]
pub struct Corpse {
    pub shards: Vec<Shard>,
    pub age: f32,
    pub lifetime: f32,
}

impl Corpse {
    /// Burst `panel` into a grid of shards that fly out from `impact` (as
    /// fractions of the panel from its top-left corner).
    pub fn burst(panel: &Panel, impact: Vec2, seed: u32) -> Self {
        let mut rng = Rng(seed.wrapping_mul(2_654_435_761) | 1);
        let columns = SHARD_COLUMNS;
        let aspect = panel.size.y / panel.size.x;
        let rows = ((columns as f32 * aspect).round() as usize).clamp(3, 12);
        let cell = Vec2::new(1.0 / columns as f32, 1.0 / rows as f32);
        // Panel coordinates: x right, y up; picture fractions run downwards.
        let to_local = Vec2::new(panel.size.x, -panel.size.y);
        let impact_local = (impact - 0.5) * to_local;

        let mut shards = Vec::with_capacity(columns * rows);
        for row in 0..rows {
            for column in 0..columns {
                let uv_min = Vec2::new(column as f32, row as f32) * cell;
                let local = (uv_min + cell / 2.0 - 0.5) * to_local;

                // Away from the impact, harder the nearer to it.
                let away = local - impact_local;
                let distance = away.length();
                let direction = if distance > 1e-4 {
                    away / distance
                } else {
                    Vec2::new(rng.signed(), rng.signed()).normalize_or(Vec2::Y)
                };
                let speed = (1.2 + 3.5 / (1.0 + 4.0 * distance)) * (0.7 + 0.6 * rng.unit());
                // Mostly away from the viewer, as the bullet came from them.
                let local_velocity = direction.extend(0.0) * speed * 0.6 - Vec3::Z * speed * 0.7;
                let vel = panel.rotation * local_velocity + Vec3::Y * (1.0 + 2.0 * rng.unit());

                shards.push(Shard {
                    pos: panel.center + panel.rotation * local.extend(0.0),
                    rot: panel.rotation,
                    size: panel.size * cell,
                    uv_min,
                    uv_max: uv_min + cell,
                    vel,
                    spin: Vec3::new(rng.signed(), rng.signed(), rng.signed()) * 9.0,
                    age: 0.0,
                    life: 2.2 + 0.6 * rng.unit(),
                });
            }
        }
        let lifetime = shards.iter().map(|shard| shard.life).fold(0.0, f32::max);
        Self {
            shards,
            age: 0.0,
            lifetime,
        }
    }

    pub fn step(&mut self, dt: f32) {
        self.age += dt;
        for shard in &mut self.shards {
            shard.step(dt);
        }
    }

    /// Whether every shard has faded and the corpse can go.
    pub fn is_done(&self) -> bool {
        self.age >= self.lifetime
    }

    /// White glow just after the burst: 1 to 0.
    pub fn flash(&self) -> f32 {
        (1.0 - self.age / BURST_FLASH_TIME).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Panel {
        Panel {
            center: Vec3::new(0.0, 1.6, 0.0),
            rotation: Quat::IDENTITY,
            size: Vec2::new(1.0, 0.75),
        }
    }

    #[test]
    fn a_dropped_shard_lands_and_bounces() {
        let mut shard = Shard {
            pos: Vec3::new(0.0, 1.0, 0.0),
            rot: Quat::IDENTITY,
            size: Vec2::splat(0.1),
            uv_min: Vec2::ZERO,
            uv_max: Vec2::ONE,
            vel: Vec3::ZERO,
            spin: Vec3::ZERO,
            age: 0.0,
            life: 10.0,
        };
        let (mut landed, mut bounced) = (false, false);
        for _ in 0..240 {
            shard.step(1.0 / 60.0);
            assert!(
                shard.pos.y >= shard.radius() - 1e-5,
                "went through the ground"
            );
            if shard.pos.y <= shard.radius() + 1e-5 && shard.vel.y > 0.0 {
                bounced = true;
            }
            landed |= shard.pos.y <= shard.radius() + 1e-5;
        }
        assert!(landed && bounced);
        // And came to rest.
        assert_eq!(shard.vel.y, 0.0);
        assert!((shard.pos.y - shard.radius()).abs() < 1e-4);
    }

    #[test]
    fn shards_fly_away_from_the_impact() {
        let corpse = Corpse::burst(&panel(), Vec2::new(0.1, 0.5), 7);
        let leftmost = corpse
            .shards
            .iter()
            .min_by(|a, b| a.pos.x.total_cmp(&b.pos.x));
        let rightmost = corpse
            .shards
            .iter()
            .max_by(|a, b| a.pos.x.total_cmp(&b.pos.x));
        assert!(rightmost.unwrap().vel.x > leftmost.unwrap().vel.x);
        // Going away from a viewer standing in front (+Z).
        assert!(corpse.shards.iter().all(|shard| shard.vel.z < 0.0));
    }

    #[test]
    fn a_burst_tiles_the_whole_picture() {
        let corpse = Corpse::burst(&panel(), Vec2::splat(0.5), 1);
        let area: f32 = corpse
            .shards
            .iter()
            .map(|s| (s.uv_max - s.uv_min).element_product())
            .sum();
        assert!((area - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_corpse_ends_when_its_lifetime_is_up() {
        let mut corpse = Corpse::burst(&panel(), Vec2::splat(0.5), 3);
        assert!(!corpse.is_done());
        let mut elapsed = 0.0;
        while !corpse.is_done() {
            corpse.step(1.0 / 60.0);
            elapsed += 1.0 / 60.0;
            assert!(elapsed < 10.0, "never ended");
        }
        assert!(elapsed >= corpse.lifetime);
        assert!(corpse.shards.iter().all(|shard| shard.alpha() == 0.0));
    }

    #[test]
    fn recoil_kicks_up_and_settles() {
        let mut gun = Gun::default();
        gun.fire();
        let (mut peak, mut total) = (0.0f32, 0.0);
        for _ in 0..120 {
            total += gun.step(1.0 / 60.0);
            peak = peak.max(gun.pitch_offset());
        }
        assert!(peak > 0.01, "no visible kick: {peak}");
        assert!(gun.pitch_offset().abs() < 1e-3, "did not settle");
        // The deltas handed to the camera add up to the offset.
        assert!((total - gun.pitch_offset()).abs() < 1e-5);
        assert_eq!(gun.flash(), 0.0);
        assert_eq!(gun.kick(), 0.0);
    }

    #[test]
    fn a_shot_window_rocks_then_rests() {
        let mut shot = Shot::new(Vec2::new(0.9, 0.2));
        let hit = shot.pose();
        assert!(hit.push > 0.1 && hit.flash == 1.0 && hit.yaw > 0.0);
        shot.age = 1.5;
        let later = shot.pose();
        assert!(later.push < 1e-3 && later.yaw.abs() < 1e-3 && later.flash == 0.0);
    }

    #[test]
    fn three_hits_kill_and_two_do_not() {
        let mut damage = Damage::default();
        assert!(!damage.hit(Vec2::new(0.2, 0.3)));
        assert!(!damage.hit(Vec2::new(0.8, 0.3)));
        assert_eq!(damage.kill_impact(), None);
        assert!(damage.hit(Vec2::new(0.5, 0.6)));
        assert_eq!(damage.kill_impact(), Some(Vec2::new(0.5, 0.6)));
    }

    #[test]
    fn a_faded_hole_no_longer_counts() {
        let mut damage = Damage::default();
        damage.hit(Vec2::splat(0.5));
        damage.step(HOLE_HOLD + HOLE_FADE + 0.1);
        assert!(damage.holes.is_empty());
        // Two fresh hits on top of a healed one are not enough.
        assert!(!damage.hit(Vec2::splat(0.4)));
        assert!(!damage.hit(Vec2::splat(0.6)));
        assert!(damage.hit(Vec2::splat(0.5)));
    }

    #[test]
    fn a_hole_still_fading_still_counts() {
        let mut damage = Damage::default();
        damage.hit(Vec2::splat(0.5));
        damage.step(HOLE_HOLD + HOLE_FADE / 2.0);
        assert_eq!(damage.holes.len(), 1);
        assert!(!damage.hit(Vec2::splat(0.4)));
        assert!(damage.hit(Vec2::splat(0.6)));
    }

    #[test]
    fn holes_fade_on_schedule() {
        let mut hole = Hole {
            at: Vec2::ZERO,
            age: 0.0,
        };
        assert_eq!(hole.alpha(), 1.0);
        hole.age = HOLE_HOLD;
        assert_eq!(hole.alpha(), 1.0);
        hole.age = HOLE_HOLD + HOLE_FADE / 2.0;
        assert!((hole.alpha() - 0.5).abs() < 1e-6);
        hole.age = HOLE_HOLD + HOLE_FADE;
        assert_eq!(hole.alpha(), 0.0);
    }

    #[test]
    fn surviving_a_killing_shot_clears_the_holes() {
        let mut damage = Damage::default();
        damage.hit(Vec2::splat(0.3));
        damage.hit(Vec2::splat(0.4));
        assert!(damage.hit(Vec2::splat(0.5)));
        damage.step(SURVIVAL_TIME - 0.5);
        assert!(damage.kill_impact().is_some());
        damage.step(1.0);
        assert!(damage.kill_impact().is_none() && damage.holes.is_empty());
        assert!(!damage.hit(Vec2::splat(0.5)), "back to full health");
    }

    #[test]
    fn a_kill_freezes_effects_briefly() {
        let mut combat = Combat::default();
        assert_eq!(combat.effects_dt(0.016), 0.016);
        combat.kill();
        let mut frozen = 0;
        while combat.effects_dt(0.016) == 0.0 {
            frozen += 1;
            assert!(frozen < 100);
        }
        assert!((2..=8).contains(&frozen));
    }
}
