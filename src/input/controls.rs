//! The rules of the controls, free of any input or windowing types: which mode
//! we are in, what the movement keys mean, how far the mouse turns the view.

use glam::Vec2;

/// Who the keyboard and mouse are talking to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// They move the viewer around the world.
    Walk,
    /// They belong to an application (or, with no window focused, to nobody).
    Interact,
}

/// Keys by position on the keyboard (Linux evdev codes), so that movement is
/// under the same fingers whatever the layout.
pub mod key {
    pub const ENTER: u32 = 28;
    pub const GRAVE: u32 = 41;
    pub const SLASH: u32 = 53;
    pub const W: u32 = 17;
    pub const A: u32 = 30;
    pub const S: u32 = 31;
    pub const D: u32 = 32;
    pub const LEFT_SHIFT: u32 = 42;
    pub const UP: u32 = 103;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const DOWN: u32 = 108;
}

/// Walking speed in metres per second.
const WALK_SPEED: f32 = 3.0;
/// How much faster holding the run key makes you.
const RUN_FACTOR: f32 = 2.5;
/// Radians of turn per logical pixel of pointer movement.
const LOOK_SENSITIVITY: f32 = 0.0022;

/// Which movement keys are held.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MoveKeys {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    run: bool,
}

impl MoveKeys {
    /// Record a key going down or up. Returns whether it is a movement key.
    pub fn set(&mut self, evdev_code: u32, pressed: bool) -> bool {
        let held = match evdev_code {
            key::W | key::UP => &mut self.forward,
            key::S | key::DOWN => &mut self.back,
            key::A | key::LEFT => &mut self.left,
            key::D | key::RIGHT => &mut self.right,
            key::LEFT_SHIFT => &mut self.run,
            _ => return false,
        };
        *held = pressed;
        true
    }

    pub fn release_all(&mut self) {
        *self = Self::default();
    }

    /// How far to move in `dt` seconds: x to the right, y forwards, in metres.
    pub fn step(&self, dt: f32) -> Vec2 {
        let axis = |positive: bool, negative: bool| f32::from(positive) - f32::from(negative);
        let direction = Vec2::new(axis(self.right, self.left), axis(self.forward, self.back));
        let speed = if self.run {
            WALK_SPEED * RUN_FACTOR
        } else {
            WALK_SPEED
        };
        // Normalising keeps diagonal movement from being faster.
        direction.normalize_or_zero() * speed * dt
    }
}

/// The (yaw, pitch) change, in radians, for the pointer moving `motion`
/// logical pixels (x right, y down). Moving right turns right, which is
/// negative yaw; moving down looks down, which is negative pitch.
pub fn look_delta(motion: Vec2) -> (f32, f32) {
    (-motion.x * LOOK_SENSITIVITY, -motion.y * LOOK_SENSITIVITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_keys_means_no_movement() {
        assert_eq!(MoveKeys::default().step(1.0), Vec2::ZERO);
    }

    #[test]
    fn keys_map_to_directions() {
        let mut keys = MoveKeys::default();
        assert!(keys.set(key::W, true));
        assert_eq!(keys.step(1.0), Vec2::new(0.0, WALK_SPEED));

        assert!(keys.set(key::W, false));
        assert!(keys.set(key::A, true));
        assert_eq!(keys.step(1.0), Vec2::new(-WALK_SPEED, 0.0));

        assert!(keys.set(key::LEFT, false));
        assert!(keys.set(key::DOWN, true));
        assert_eq!(keys.step(0.5), Vec2::new(0.0, -WALK_SPEED * 0.5));
    }

    #[test]
    fn opposite_keys_cancel() {
        let mut keys = MoveKeys::default();
        keys.set(key::W, true);
        keys.set(key::S, true);
        assert_eq!(keys.step(1.0), Vec2::ZERO);
    }

    #[test]
    fn diagonal_movement_is_not_faster() {
        let mut keys = MoveKeys::default();
        keys.set(key::W, true);
        keys.set(key::D, true);
        let step = keys.step(1.0);
        assert!((step.length() - WALK_SPEED).abs() < 1e-5);
        assert!(step.x > 0.0 && step.y > 0.0);
    }

    #[test]
    fn run_key_speeds_up_but_does_not_move_by_itself() {
        let mut keys = MoveKeys::default();
        assert!(keys.set(key::LEFT_SHIFT, true));
        assert_eq!(keys.step(1.0), Vec2::ZERO);
        keys.set(key::W, true);
        assert_eq!(keys.step(1.0), Vec2::new(0.0, WALK_SPEED * RUN_FACTOR));
    }

    #[test]
    fn other_keys_are_not_movement_keys() {
        let mut keys = MoveKeys::default();
        assert!(!keys.set(key::ENTER, true));
        assert_eq!(keys, MoveKeys::default());
    }

    #[test]
    fn release_all_stops_movement() {
        let mut keys = MoveKeys::default();
        keys.set(key::W, true);
        keys.set(key::LEFT_SHIFT, true);
        keys.release_all();
        assert_eq!(keys.step(1.0), Vec2::ZERO);
    }

    #[test]
    fn mouse_right_turns_right_and_mouse_down_looks_down() {
        let (yaw, pitch) = look_delta(Vec2::new(10.0, 5.0));
        assert!(yaw < 0.0 && pitch < 0.0);
        let (yaw, pitch) = look_delta(Vec2::new(-10.0, -5.0));
        assert!(yaw > 0.0 && pitch > 0.0);
    }
}
