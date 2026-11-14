//! The 3D world: where the camera is and where each window's panel stands.
//!
//! Pure maths and data. Nothing here knows about GL or Wayland, so all of it
//! can be unit tested.
//!
//! Conventions: right-handed, +Y up, distances in metres. A camera with zero
//! yaw and pitch looks along -Z.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::{Mat4, Quat, Vec2, Vec3};

pub mod combat;

/// How many logical pixels of a window make one metre of panel. At 1000, a
/// panel about a metre away shows roughly one window pixel per screen pixel.
pub const PIXELS_PER_METRE: f32 = 1000.0;

/// Height of the viewer's eyes above the ground.
pub const EYE_HEIGHT: f32 = 1.6;

/// How far up or down the camera can look. Stopping just short of vertical
/// keeps "forward" well defined.
const MAX_PITCH: f32 = FRAC_PI_2 - 0.02;

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
    pub fn new(position: Vec3, yaw: f32, pitch: f32) -> Self {
        let mut camera = Self {
            position,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 60f32.to_radians(),
            near: 0.05,
            far: 500.0,
        };
        camera.turn(yaw, pitch);
        camera
    }

    fn orientation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }

    /// Unit vector pointing where the camera looks.
    pub fn forward(&self) -> Vec3 {
        self.orientation() * Vec3::NEG_Z
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

    /// Rotate the view: positive `yaw` turns left, positive `pitch` looks up.
    pub fn turn(&mut self, yaw: f32, pitch: f32) {
        // Keep yaw in (-PI, PI] so it never loses precision by growing.
        self.yaw = PI - (PI - (self.yaw + yaw)).rem_euclid(TAU);
        self.pitch = (self.pitch + pitch).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Move over the ground by `step` metres: `step.x` to the camera's right,
    /// `step.y` the way it faces. Looking up or down does not change the
    /// direction or speed of travel.
    pub fn walk(&mut self, step: Vec2) {
        self.position += Quat::from_rotation_y(self.yaw) * Vec3::new(step.x, 0.0, -step.y);
    }

    /// The ray from the eye through a point on the screen, given in
    /// normalised device coordinates: (0, 0) is the centre, +x right, +y up,
    /// and the edges are at +-1.
    pub fn ray_through(&self, ndc: Vec2, aspect: f32) -> Ray {
        let half_height = (self.fov_y / 2.0).tan();
        let in_view = Vec3::new(ndc.x * half_height * aspect, ndc.y * half_height, -1.0);
        Ray {
            origin: self.position,
            direction: (self.orientation() * in_view).normalize(),
        }
    }
}

/// A half-line from `origin` along unit vector `direction`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

/// Where a ray met a panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Distance along the ray, in metres.
    pub distance: f32,
    /// Position on the panel as fractions of its size, measured from its
    /// top-left corner as seen from the front: x to the right, y downwards.
    /// Multiply by the window's size for window coordinates.
    pub uv: Vec2,
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

    /// The panel covering part of this panel's plane: the rectangle from
    /// `min` to `max`, given like `Hit::uv` as fractions of this panel's size
    /// from its top-left corner. The fractions may lie outside 0 to 1, which
    /// is how a window's popups, hanging off its edges, get their place.
    pub fn region(&self, min: Vec2, max: Vec2) -> Panel {
        let middle = (min + max) / 2.0;
        let offset = Vec3::new(
            (middle.x - 0.5) * self.size.x,
            (0.5 - middle.y) * self.size.y,
            0.0,
        );
        Panel {
            center: self.center + self.rotation * offset,
            rotation: self.rotation,
            size: (max - min) * self.size,
        }
    }

    /// Where `ray` meets the plane the panel lies in, approaching from the
    /// front. The hit's `uv` is outside 0 to 1 if that is beyond the panel's
    /// edges. A ray arriving from behind misses: the back shows the window
    /// mirrored, and pointing at that should not count as pointing at it.
    pub fn intersect_plane(&self, ray: &Ray) -> Option<Hit> {
        // Work in the panel's own frame, where it lies in the z = 0 plane,
        // centred on the origin, with its front towards +z.
        let to_local = self.rotation.inverse();
        let origin = to_local * (ray.origin - self.center);
        let direction = to_local * ray.direction;

        // Must be in front of the panel and heading towards it.
        if origin.z <= 0.0 || direction.z >= 0.0 {
            return None;
        }
        let distance = -origin.z / direction.z;
        let point = origin + direction * distance;
        Some(Hit {
            distance,
            uv: Vec2::new(point.x / self.size.x + 0.5, 0.5 - point.y / self.size.y),
        })
    }

    /// Where `ray` meets the front of the panel itself, if it does.
    pub fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.intersect_plane(ray)
            .filter(|hit| hit.uv.cmpge(Vec2::ZERO).all() && hit.uv.cmple(Vec2::ONE).all())
    }
}

/// The panel `ray` reaches first, as its index in `panels`, and where.
///
/// `solid` says whether a hit counts: a panel can have holes (the gaps
/// between a window and the menus hanging off it), and a ray through a hole
/// carries on to whatever is behind.
pub fn pick<'a>(
    ray: &Ray,
    panels: impl IntoIterator<Item = &'a Panel>,
    mut solid: impl FnMut(usize, &Hit) -> bool,
) -> Option<(usize, Hit)> {
    panels
        .into_iter()
        .enumerate()
        .filter_map(|(index, panel)| Some((index, panel.intersect(ray)?)))
        .filter(|(index, hit)| solid(*index, hit))
        .min_by(|(_, a), (_, b)| a.distance.total_cmp(&b.distance))
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

    fn close2(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < EPS
    }

    /// A 2 m by 1 m panel centred 3 m in front of the origin, facing it.
    fn panel_ahead() -> Panel {
        Panel {
            center: Vec3::new(0.0, 0.0, -3.0),
            rotation: Quat::IDENTITY,
            size: Vec2::new(2.0, 1.0),
        }
    }

    fn ray(origin: Vec3, toward: Vec3) -> Ray {
        Ray {
            origin,
            direction: (toward - origin).normalize(),
        }
    }

    #[test]
    fn camera_orientation_follows_the_conventions() {
        let level = Camera::new(Vec3::ZERO, 0.0, 0.0);
        assert!(close(level.forward(), Vec3::NEG_Z));

        let turned_left = Camera::new(Vec3::ZERO, FRAC_PI_2, 0.0);
        assert!(close(turned_left.forward(), Vec3::NEG_X));

        let looking_up = Camera::new(Vec3::ZERO, 0.0, 0.5);
        assert!(looking_up.forward().y > 0.0);
    }

    #[test]
    fn point_straight_ahead_projects_to_the_centre_of_the_view() {
        let camera = Camera::new(Vec3::new(1.0, 2.0, 3.0), 0.7, -0.3);
        let target = camera.position + camera.forward() * 4.0;
        let ndc = camera.view_projection(16.0 / 9.0).project_point3(target);
        assert!(ndc.x.abs() < EPS && ndc.y.abs() < EPS, "{ndc}");
        assert!((-1.0..=1.0).contains(&ndc.z));
    }

    #[test]
    fn points_right_of_and_above_the_view_axis_land_right_and_up() {
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let ndc = camera
            .view_projection(1.0)
            .project_point3(Vec3::new(0.5, 0.25, -2.0));
        assert!(ndc.x > 0.0 && ndc.y > 0.0, "{ndc}");
    }

    #[test]
    fn turning_clamps_pitch_and_wraps_yaw() {
        let mut camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        camera.turn(0.0, 10.0);
        assert!(camera.pitch < FRAC_PI_2 && camera.pitch > 1.5);
        camera.turn(0.0, -20.0);
        assert!(camera.pitch > -FRAC_PI_2 && camera.pitch < -1.5);

        let mut camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        for _ in 0..100 {
            camera.turn(1.0, 0.0);
        }
        assert!(camera.yaw.abs() <= PI + EPS);
        // 100 radians is 15 full turns plus 5.752 radians, which is -0.531.
        assert!(
            (camera.yaw - (100.0 - 16.0 * TAU)).abs() < 1e-3,
            "{}",
            camera.yaw
        );
    }

    #[test]
    fn walking_follows_yaw_but_not_pitch() {
        let mut camera = Camera::new(Vec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 1.0);
        camera.walk(Vec2::new(0.0, 2.0));
        assert!(close(camera.position, Vec3::new(0.0, EYE_HEIGHT, -2.0)));
        camera.walk(Vec2::new(1.0, 0.0));
        assert!(close(camera.position, Vec3::new(1.0, EYE_HEIGHT, -2.0)));

        // Facing left (-X): forward goes to -X, right goes to -Z.
        let mut camera = Camera::new(Vec3::ZERO, FRAC_PI_2, 0.0);
        camera.walk(Vec2::new(0.0, 1.0));
        assert!(close(camera.position, Vec3::NEG_X));
        camera.walk(Vec2::new(1.0, 0.0));
        assert!(close(camera.position, Vec3::new(-1.0, 0.0, -1.0)));
    }

    #[test]
    fn ray_through_agrees_with_the_projection() {
        let camera = Camera::new(Vec3::new(1.0, 2.0, 3.0), 0.7, -0.3);
        let aspect = 16.0 / 9.0;

        let centre = camera.ray_through(Vec2::ZERO, aspect);
        assert!(close(centre.origin, camera.position));
        assert!(close(centre.direction, camera.forward()));

        // A point along the ray through some screen position must project
        // back to that screen position.
        let ndc = Vec2::new(0.4, -0.7);
        let ray = camera.ray_through(ndc, aspect);
        let projected = camera
            .view_projection(aspect)
            .project_point3(ray.origin + ray.direction * 5.0);
        assert!(close2(projected.truncate(), ndc), "{projected}");
    }

    #[test]
    fn facing_panel_points_its_front_at_the_viewer() {
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
    fn ray_at_the_centre_hits_the_middle_of_the_panel() {
        let hit = panel_ahead()
            .intersect(&ray(Vec3::ZERO, Vec3::new(0.0, 0.0, -3.0)))
            .expect("hit");
        assert!((hit.distance - 3.0).abs() < EPS);
        assert!(close2(hit.uv, Vec2::new(0.5, 0.5)));
    }

    #[test]
    fn hit_coordinates_start_at_the_top_left_as_seen_from_the_front() {
        let panel = panel_ahead();
        // Seen from the origin the panel spans x in [-1, 1] and y in
        // [-0.5, 0.5]; its top-left corner is at (-1, 0.5).
        let near_top_left = panel
            .intersect(&ray(Vec3::ZERO, Vec3::new(-0.9, 0.4, -3.0)))
            .expect("hit");
        assert!(close2(near_top_left.uv, Vec2::new(0.05, 0.1)));

        let near_bottom_right = panel
            .intersect(&ray(Vec3::ZERO, Vec3::new(0.9, -0.4, -3.0)))
            .expect("hit");
        assert!(close2(near_bottom_right.uv, Vec2::new(0.95, 0.9)));
    }

    #[test]
    fn hit_coordinates_follow_a_turned_panel() {
        // A panel to the viewer's right, facing back towards -X. Its left
        // edge, as seen from the front, is the one further along -Z.
        let panel = Panel::facing(Vec3::new(3.0, 0.0, 0.0), Vec3::ZERO, Vec2::new(2.0, 1.0));
        let hit = panel
            .intersect(&ray(Vec3::ZERO, Vec3::new(3.0, 0.0, -0.5)))
            .expect("hit");
        assert!(close2(hit.uv, Vec2::new(0.25, 0.5)), "{}", hit.uv);
    }

    #[test]
    fn rays_that_should_miss_do() {
        let panel = panel_ahead();
        // Past the right edge and past the top edge.
        assert!(
            panel
                .intersect(&ray(Vec3::ZERO, Vec3::new(1.1, 0.0, -3.0)))
                .is_none()
        );
        assert!(
            panel
                .intersect(&ray(Vec3::ZERO, Vec3::new(0.0, 0.6, -3.0)))
                .is_none()
        );
        // Pointing away from it.
        assert!(
            panel
                .intersect(&ray(Vec3::ZERO, Vec3::new(0.0, 0.0, 3.0)))
                .is_none()
        );
        // Parallel to it.
        assert!(panel.intersect(&ray(Vec3::ZERO, Vec3::X)).is_none());
        // From behind, pointing at its back.
        let behind = Vec3::new(0.0, 0.0, -6.0);
        assert!(
            panel
                .intersect(&ray(behind, Vec3::new(0.0, 0.0, -3.0)))
                .is_none()
        );
    }

    #[test]
    fn plane_hits_continue_past_the_panel_edges() {
        let panel = panel_ahead();
        // One metre beyond the right edge, level with the middle.
        let toward = Vec3::new(2.0, 0.0, -3.0);
        assert!(panel.intersect(&ray(Vec3::ZERO, toward)).is_none());
        let hit = panel
            .intersect_plane(&ray(Vec3::ZERO, toward))
            .expect("the plane is hit");
        assert!(close2(hit.uv, Vec2::new(1.5, 0.5)));
    }

    #[test]
    fn region_covers_the_requested_part_of_the_plane() {
        let panel = panel_ahead();

        let whole = panel.region(Vec2::ZERO, Vec2::ONE);
        assert!(close(whole.center, panel.center));
        assert!(close2(whole.size, panel.size));

        // A menu hanging below the bottom-left corner: the left quarter of
        // the width, from the bottom edge down by half the height again.
        let menu = panel.region(Vec2::new(0.0, 1.0), Vec2::new(0.25, 1.5));
        assert!(close2(menu.size, Vec2::new(0.5, 0.5)));
        assert!(close(menu.center, Vec3::new(-0.75, -0.75, -3.0)));

        // A point seen through the region maps back to the same place on the
        // original panel's plane.
        let through = ray(Vec3::ZERO, Vec3::new(-0.75, -0.75, -3.0));
        let on_menu = menu.intersect(&through).expect("hit");
        assert!(close2(on_menu.uv, Vec2::new(0.5, 0.5)));
        let on_panel = panel.intersect_plane(&through).expect("hit");
        assert!(close2(on_panel.uv, Vec2::new(0.125, 1.25)));
    }

    #[test]
    fn region_follows_a_turned_panel() {
        let panel = Panel::facing(Vec3::new(3.0, 0.0, 0.0), Vec3::ZERO, Vec2::new(2.0, 1.0));
        // The right half, as seen from the front. Seen from the origin this
        // panel's right is towards +Z.
        let right_half = panel.region(Vec2::new(0.5, 0.0), Vec2::ONE);
        assert!(close(right_half.center, Vec3::new(3.0, 0.0, 0.5)));
        assert!(close2(right_half.size, Vec2::new(1.0, 1.0)));
    }

    #[test]
    fn pick_returns_the_nearest_panel_hit() {
        let far = panel_ahead();
        let near = Panel {
            center: Vec3::new(0.0, 0.0, -1.0),
            ..panel_ahead()
        };
        let aside = Panel {
            center: Vec3::new(10.0, 0.0, -1.0),
            ..panel_ahead()
        };
        let straight = ray(Vec3::ZERO, Vec3::NEG_Z);

        let (index, hit) = pick(&straight, [&far, &aside, &near], |_, _| true).expect("hit");
        assert_eq!(index, 2);
        assert!((hit.distance - 1.0).abs() < EPS);

        assert!(pick(&straight, [&aside], |_, _| true).is_none());
        assert!(pick(&straight, [], |_, _| true).is_none());
    }

    #[test]
    fn pick_passes_through_holes() {
        let far = panel_ahead();
        let near = Panel {
            center: Vec3::new(0.0, 0.0, -1.0),
            ..panel_ahead()
        };
        let straight = ray(Vec3::ZERO, Vec3::NEG_Z);

        // The near panel has a hole where the ray meets it.
        let (index, hit) = pick(&straight, [&far, &near], |index, _| index != 1).expect("hit");
        assert_eq!(index, 0);
        assert!((hit.distance - 3.0).abs() < EPS);

        assert!(pick(&straight, [&far, &near], |_, _| false).is_none());
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
