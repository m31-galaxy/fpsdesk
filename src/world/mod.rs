//! The 3D world: where the camera is and where each window's panel stands.
//!
//! Pure maths and data. Nothing here knows about GL or Wayland, so all of it
//! can be unit tested.
//!
//! Conventions: right-handed, +Y up, distances in metres. A camera with zero
//! yaw and pitch looks along -Z.

use glam::{Mat4, Quat, Vec2, Vec3};

/// How many logical pixels of a window make one metre of panel. At 1000, a
/// panel about a metre away shows roughly one window pixel per screen pixel.
pub const PIXELS_PER_METRE: f32 = 1000.0;

/// Height of the viewer's eyes above the ground.
pub const EYE_HEIGHT: f32 = 1.6;

/// The viewer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    /// Rotation about +Y in radians; positive turns left.
    pub yaw: f32,
    /// Rotation about the camera's right axis in radians; positive looks up.
    pub pitch: f32,
    /// Vertical field of view in radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    /// A camera at `position` aimed at `target`.
    pub fn looking_at(position: Vec3, target: Vec3) -> Self {
        let dir = (target - position).normalize();
        Self {
            position,
            yaw: (-dir.x).atan2(-dir.z),
            pitch: dir.y.asin(),
            fov_y: 60f32.to_radians(),
            near: 0.05,
            far: 200.0,
        }
    }

    /// Unit vector pointing where the camera looks.
    pub fn forward(&self) -> Vec3 {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch) * Vec3::NEG_Z
    }

    /// World space to camera space.
    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_to_mat4(self.position, self.forward(), Vec3::Y)
    }

    /// Camera space to GL clip space, for a viewport `aspect` wide per unit high.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        glam::camera::rh::proj::opengl::perspective(self.fov_y, aspect, self.near, self.far)
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }
}

/// A window standing in the world: a flat rectangle showing its contents on
/// its front, the side that `rotation` turns +Z towards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Panel {
    pub center: Vec3,
    pub rotation: Quat,
    /// Width and height in metres.
    pub size: Vec2,
}

impl Panel {
    /// An upright panel at `center` turned to face `viewer`.
    pub fn facing(center: Vec3, viewer: Vec3, size: Vec2) -> Self {
        let toward = viewer - center;
        Self {
            center,
            rotation: Quat::from_rotation_y(toward.x.atan2(toward.z)),
            size,
        }
    }

    /// Maps the unit quad (corners at +-0.5 in x and y, z = 0) onto the panel.
    pub fn model(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.size.extend(1.0), self.rotation, self.center)
    }
}

/// Where the `slot`th window goes while there is no real placement logic:
/// on an arc around the origin at eye height, facing inwards, filling outwards
/// from straight ahead (slot 0), alternating right and left.
pub fn arc_slot(slot: usize) -> Panel {
    const RADIUS: f32 = 1.6;
    const STEP: f32 = 40.0;

    let steps = slot.div_ceil(2) as f32;
    let side = if slot % 2 == 1 { 1.0 } else { -1.0 };
    let angle = (side * steps * STEP).to_radians();

    let center = Vec3::new(RADIUS * angle.sin(), EYE_HEIGHT, -RADIUS * angle.cos());
    Panel::facing(center, Vec3::new(0.0, EYE_HEIGHT, 0.0), Vec2::ONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    /// The direction a panel shows its contents towards.
    fn front(panel: &Panel) -> Vec3 {
        panel.rotation * Vec3::Z
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < EPS
    }

    #[test]
    fn default_orientation_looks_down_negative_z() {
        let camera = Camera::looking_at(Vec3::ZERO, Vec3::new(0.0, 0.0, -5.0));
        assert!(camera.yaw.abs() < EPS && camera.pitch.abs() < EPS);
        assert!(close(camera.forward(), Vec3::NEG_Z));
    }

    #[test]
    fn looking_at_aims_forward_at_the_target() {
        let position = Vec3::new(1.0, 2.0, 3.0);
        let target = Vec3::new(-4.0, 0.5, -2.0);
        let camera = Camera::looking_at(position, target);
        assert!(close(camera.forward(), (target - position).normalize()));
    }

    #[test]
    fn target_projects_to_the_centre_of_the_view() {
        let target = Vec3::new(-4.0, 0.5, -2.0);
        let camera = Camera::looking_at(Vec3::new(1.0, 2.0, 3.0), target);
        let ndc = camera.view_projection(16.0 / 9.0).project_point3(target);
        assert!(ndc.x.abs() < EPS && ndc.y.abs() < EPS, "{ndc}");
        assert!((-1.0..=1.0).contains(&ndc.z));
    }

    #[test]
    fn points_right_of_and_above_the_view_axis_land_right_and_up() {
        let camera = Camera::looking_at(Vec3::ZERO, Vec3::NEG_Z);
        let ndc = camera
            .view_projection(1.0)
            .project_point3(Vec3::new(0.5, 0.25, -2.0));
        assert!(ndc.x > 0.0 && ndc.y > 0.0, "{ndc}");
    }

    #[test]
    fn facing_panel_points_its_normal_at_the_viewer() {
        let center = Vec3::new(2.0, 1.0, -3.0);
        let viewer = Vec3::new(-1.0, 1.0, 4.0);
        let panel = Panel::facing(center, viewer, Vec2::ONE);
        assert!(close(front(&panel), (viewer - center).normalize()));
    }

    #[test]
    fn model_maps_unit_quad_corners_to_panel_corners() {
        let panel = Panel {
            center: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::IDENTITY,
            size: Vec2::new(2.0, 1.0),
        };
        let top_right = panel.model().transform_point3(Vec3::new(0.5, 0.5, 0.0));
        assert!(close(top_right, Vec3::new(2.0, 2.5, 3.0)));
    }

    #[test]
    fn arc_slots_face_the_origin_and_do_not_coincide() {
        let eye = Vec3::new(0.0, EYE_HEIGHT, 0.0);
        let slots: Vec<Panel> = (0..5).map(arc_slot).collect();

        assert!(close(slots[0].center, Vec3::new(0.0, EYE_HEIGHT, -1.6)));
        // Slot 1 is to the right of straight ahead, slot 2 to the left.
        assert!(slots[1].center.x > 0.0 && slots[2].center.x < 0.0);

        for (i, panel) in slots.iter().enumerate() {
            assert!(close(front(panel), (eye - panel.center).normalize()));
            for other in &slots[i + 1..] {
                assert!((panel.center - other.center).length() > 0.5);
            }
        }
    }
}
