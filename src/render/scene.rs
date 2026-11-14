//! Pass 2: draw the 3D world into a framebuffer of our own.
//!
//! This is the only place that issues raw GL draw calls. The context is shared
//! with Smithay's renderer, whose GL state is an implementation detail we must
//! not disturb, so `render` ends by putting back everything it changed that
//! Smithay does not set for itself (see `restore_smithay_state`).

use std::error::Error;

use glam::{Mat4, Quat, Vec3};
use smithay::{
    backend::renderer::gles::{
        GlesRenderer, GlesTexture,
        ffi::{self, Gles2},
    },
    utils::{Physical, Size},
};

use super::gl;
use crate::world::Camera;

// The palette. The ground fades into `HORIZON` with distance, which is also
// the sky's colour at eye level, so the two meet without a seam.
const ZENITH: [f32; 3] = [0.015, 0.02, 0.05];
const HORIZON: [f32; 3] = [0.10, 0.13, 0.20];
const GROUND: [f32; 3] = [0.045, 0.055, 0.075];
const GRID_LINE: [f32; 3] = [0.22, 0.30, 0.42];

/// Side of the square of ground drawn around the viewer, in metres. Twice the
/// camera's far distance, so its edge is never in view.
const GROUND_SIZE: f32 = 1000.0;

const QUAD_VERTEX: &str = "#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
uniform mat4 u_mvp;
out vec2 v_uv;
void main() {
    v_uv = a_uv;
    gl_Position = u_mvp * vec4(a_position, 0.0, 1.0);
}
";

const QUAD_FRAGMENT: &str = "#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_texture;
uniform vec4 u_tint;
out vec4 color;
void main() {
    color = texture(u_texture, v_uv) * u_tint;
}
";

/// One triangle covering the whole screen, with the world-space direction each
/// pixel looks in.
const SKY_VERTEX: &str = "#version 300 es
uniform mat4 u_inverse_view_projection;
uniform vec3 u_eye;
out vec3 v_direction;
void main() {
    vec2 position = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)) * 2.0 - 1.0;
    vec4 far = u_inverse_view_projection * vec4(position, 1.0, 1.0);
    v_direction = far.xyz / far.w - u_eye;
    gl_Position = vec4(position, 1.0, 1.0);
}
";

const SKY_FRAGMENT: &str = "#version 300 es
precision highp float;
in vec3 v_direction;
uniform vec3 u_horizon;
uniform vec3 u_zenith;
out vec4 color;
void main() {
    float height = normalize(v_direction).y;
    color = vec4(mix(u_horizon, u_zenith, smoothstep(0.0, 0.5, height)), 1.0);
}
";

const GROUND_VERTEX: &str = "#version 300 es
layout(location = 0) in vec2 a_position;
uniform mat4 u_model;
uniform mat4 u_view_projection;
out vec2 v_world;
void main() {
    vec4 world = u_model * vec4(a_position, 0.0, 1.0);
    v_world = world.xz;
    gl_Position = u_view_projection * world;
}
";

/// A grid of lines every metre, with stronger ones every ten.
const GROUND_FRAGMENT: &str = "#version 300 es
precision highp float;
in vec2 v_world;
uniform vec3 u_eye;
uniform vec3 u_ground;
uniform vec3 u_line;
uniform vec3 u_horizon;
out vec4 color;

// 1 on a grid line and 0 between lines, about 1.5 pixels wide whatever the
// distance. Fades out where cells shrink towards a pixel, which would
// otherwise shimmer.
float grid(float spacing) {
    vec2 cell = v_world / spacing;
    vec2 cells_per_pixel = fwidth(cell);
    vec2 pixels_from_line = abs(fract(cell - 0.5) - 0.5) / cells_per_pixel;
    float line = 1.0 - min(min(pixels_from_line.x, pixels_from_line.y) / 1.5, 1.0);
    float density = max(cells_per_pixel.x, cells_per_pixel.y);
    return line * (1.0 - smoothstep(0.1, 0.5, density));
}

void main() {
    float lines = max(grid(1.0) * 0.5, grid(10.0));
    vec3 surface = mix(u_ground, u_line, lines);
    float visibility = exp(-distance(v_world, u_eye.xz) * 0.02);
    color = vec4(mix(u_horizon, surface, visibility), 1.0);
}
";

/// Unit quad as a triangle strip: x, y, u, v. Window textures have their first
/// row at the top, so v runs downwards.
#[rustfmt::skip]
const QUAD: [f32; 16] = [
    -0.5, -0.5,  0.0, 1.0,
     0.5, -0.5,  1.0, 1.0,
    -0.5,  0.5,  0.0, 0.0,
     0.5,  0.5,  1.0, 0.0,
];

/// One textured rectangle to draw.
pub struct Quad {
    /// Places the unit quad in the world; see `Panel::model`.
    pub model: Mat4,
    /// GL name of a 2D texture with premultiplied alpha.
    pub texture: u32,
    /// A border to draw around the quad: the placement of a slightly larger
    /// rectangle in the same plane, and its colour.
    pub outline: Option<(Mat4, [f32; 3])>,
}

struct QuadProgram {
    program: u32,
    u_mvp: i32,
    u_texture: i32,
    u_tint: i32,
}

struct SkyProgram {
    program: u32,
    u_inverse_view_projection: i32,
    u_eye: i32,
    u_horizon: i32,
    u_zenith: i32,
}

struct GroundProgram {
    program: u32,
    u_model: i32,
    u_view_projection: i32,
    u_eye: i32,
    u_ground: i32,
    u_line: i32,
    u_horizon: i32,
}

pub struct SceneRenderer {
    quad: QuadProgram,
    sky: SkyProgram,
    ground: GroundProgram,
    /// The unit quad.
    vao: u32,
    /// A single white texel, for drawing plain colours with the quad program.
    white: u32,
    target: Option<Target>,
}

/// The framebuffer the world is drawn into.
struct Target {
    size: Size<i32, Physical>,
    framebuffer: u32,
    depth: u32,
    /// Colour attachment. Wrapped as a Smithay texture so pass 3 can present
    /// it; Smithay deletes the GL texture when the last clone is dropped.
    color: GlesTexture,
}

impl SceneRenderer {
    pub fn new(renderer: &mut GlesRenderer) -> Result<Self, Box<dyn Error>> {
        renderer.with_context(|gl| unsafe {
            let version = gl::get_string(gl, ffi::VERSION);
            // "OpenGL ES 3.x ...". VAOs, depth renderbuffers and the shaders
            // above all need at least 3.0.
            let major = version
                .strip_prefix("OpenGL ES ")
                .and_then(|rest| rest.chars().next())
                .and_then(|c| c.to_digit(10));
            if major.is_none_or(|major| major < 3) {
                return Err(format!("OpenGL ES 3.0 or newer is required, got {version:?}").into());
            }

            let uniform =
                |program, name: &std::ffi::CStr| gl.GetUniformLocation(program, name.as_ptr());

            let program = gl::link_program(gl, QUAD_VERTEX, QUAD_FRAGMENT)?;
            let quad = QuadProgram {
                program,
                u_mvp: uniform(program, c"u_mvp"),
                u_texture: uniform(program, c"u_texture"),
                u_tint: uniform(program, c"u_tint"),
            };

            let program = gl::link_program(gl, SKY_VERTEX, SKY_FRAGMENT)?;
            let sky = SkyProgram {
                program,
                u_inverse_view_projection: uniform(program, c"u_inverse_view_projection"),
                u_eye: uniform(program, c"u_eye"),
                u_horizon: uniform(program, c"u_horizon"),
                u_zenith: uniform(program, c"u_zenith"),
            };

            let program = gl::link_program(gl, GROUND_VERTEX, GROUND_FRAGMENT)?;
            let ground = GroundProgram {
                program,
                u_model: uniform(program, c"u_model"),
                u_view_projection: uniform(program, c"u_view_projection"),
                u_eye: uniform(program, c"u_eye"),
                u_ground: uniform(program, c"u_ground"),
                u_line: uniform(program, c"u_line"),
                u_horizon: uniform(program, c"u_horizon"),
            };

            let mut vao = 0;
            let mut vbo = 0;
            gl.GenVertexArrays(1, &mut vao);
            gl.GenBuffers(1, &mut vbo);
            gl.BindVertexArray(vao);
            gl.BindBuffer(ffi::ARRAY_BUFFER, vbo);
            gl.BufferData(
                ffi::ARRAY_BUFFER,
                size_of_val(&QUAD) as isize,
                QUAD.as_ptr().cast(),
                ffi::STATIC_DRAW,
            );
            let stride = (4 * size_of::<f32>()) as i32;
            gl.EnableVertexAttribArray(0);
            gl.VertexAttribPointer(0, 2, ffi::FLOAT, ffi::FALSE, stride, std::ptr::null());
            gl.EnableVertexAttribArray(1);
            gl.VertexAttribPointer(
                1,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                stride,
                (2 * size_of::<f32>()) as *const _,
            );
            gl.BindVertexArray(0);
            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);

            let mut white = 0;
            gl.GenTextures(1, &mut white);
            gl.BindTexture(ffi::TEXTURE_2D, white);
            gl.TexImage2D(
                ffi::TEXTURE_2D,
                0,
                ffi::RGBA8 as i32,
                1,
                1,
                0,
                ffi::RGBA,
                ffi::UNSIGNED_BYTE,
                [0xffu8; 4].as_ptr().cast(),
            );
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_MIN_FILTER,
                ffi::NEAREST as i32,
            );
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_MAG_FILTER,
                ffi::NEAREST as i32,
            );
            gl.BindTexture(ffi::TEXTURE_2D, 0);

            Ok(Self {
                quad,
                sky,
                ground,
                vao,
                white,
                target: None,
            })
        })?
    }

    /// Draw the sky, the ground and `quads` (already ordered back to front) as
    /// seen by `camera`. Returns the texture holding the finished image; its
    /// first row is the bottom of the picture, as usual for GL framebuffers.
    pub fn render(
        &mut self,
        renderer: &mut GlesRenderer,
        size: Size<i32, Physical>,
        camera: &Camera,
        quads: &[Quad],
    ) -> Result<GlesTexture, Box<dyn Error>> {
        if self
            .target
            .as_ref()
            .is_none_or(|target| target.size != size)
        {
            if let Some(old) = self.target.take() {
                old.destroy(renderer)?;
            }
            self.target = Some(Target::new(renderer, size)?);
        }
        let target = self.target.as_ref().expect("target was just created");

        let view_projection = camera.view_projection(size.w as f32 / size.h as f32);
        let eye = camera.position;

        renderer.with_context(|gl| unsafe {
            gl.BindFramebuffer(ffi::FRAMEBUFFER, target.framebuffer);
            gl.Viewport(0, 0, size.w, size.h);
            gl.Disable(ffi::SCISSOR_TEST);
            gl.Disable(ffi::BLEND);
            gl.Clear(ffi::DEPTH_BUFFER_BIT);
            gl.BindVertexArray(self.vao);

            // The sky covers every pixel, so it doubles as the colour clear.
            gl.Disable(ffi::DEPTH_TEST);
            self.draw_sky(gl, view_projection, eye);

            gl.Enable(ffi::DEPTH_TEST);
            self.draw_ground(gl, view_projection, eye);

            // Window textures are premultiplied.
            gl.Enable(ffi::BLEND);
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
            self.draw_quads(gl, view_projection, quads);

            restore_smithay_state(gl);
        })?;

        Ok(target.color.clone())
    }

    unsafe fn draw_sky(&self, gl: &Gles2, view_projection: Mat4, eye: Vec3) {
        let sky = &self.sky;
        unsafe {
            gl.UseProgram(sky.program);
            set_mat4(gl, sky.u_inverse_view_projection, view_projection.inverse());
            set_vec3(gl, sky.u_eye, eye.to_array());
            set_vec3(gl, sky.u_horizon, HORIZON);
            set_vec3(gl, sky.u_zenith, ZENITH);
            gl.DrawArrays(ffi::TRIANGLES, 0, 3);
        }
    }

    unsafe fn draw_ground(&self, gl: &Gles2, view_projection: Mat4, eye: Vec3) {
        let ground = &self.ground;
        // Lay the unit quad flat and keep it centred under the viewer. The
        // grid is computed from world coordinates, so it stays put.
        let model = Mat4::from_scale_rotation_translation(
            Vec3::splat(GROUND_SIZE),
            Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            Vec3::new(eye.x, 0.0, eye.z),
        );
        unsafe {
            gl.UseProgram(ground.program);
            set_mat4(gl, ground.u_model, model);
            set_mat4(gl, ground.u_view_projection, view_projection);
            set_vec3(gl, ground.u_eye, eye.to_array());
            set_vec3(gl, ground.u_ground, GROUND);
            set_vec3(gl, ground.u_line, GRID_LINE);
            set_vec3(gl, ground.u_horizon, HORIZON);
            gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
        }
    }

    unsafe fn draw_quads(&self, gl: &Gles2, view_projection: Mat4, quads: &[Quad]) {
        let quad_program = &self.quad;
        unsafe {
            gl.UseProgram(quad_program.program);
            gl.ActiveTexture(ffi::TEXTURE0);
            gl.Uniform1i(quad_program.u_texture, 0);

            for quad in quads {
                if let Some((model, [r, g, b])) = quad.outline {
                    // The outline is in the same plane as its quad. Leaving it
                    // out of the depth buffer lets the quad draw over it.
                    gl.DepthMask(ffi::FALSE);
                    set_mat4(gl, quad_program.u_mvp, view_projection * model);
                    gl.Uniform4f(quad_program.u_tint, r, g, b, 1.0);
                    gl.BindTexture(ffi::TEXTURE_2D, self.white);
                    gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
                    gl.DepthMask(ffi::TRUE);
                }

                set_mat4(gl, quad_program.u_mvp, view_projection * quad.model);
                gl.Uniform4f(quad_program.u_tint, 1.0, 1.0, 1.0, 1.0);
                gl.BindTexture(ffi::TEXTURE_2D, quad.texture);
                gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
            }
        }
    }
}

unsafe fn set_mat4(gl: &Gles2, location: i32, matrix: Mat4) {
    unsafe { gl.UniformMatrix4fv(location, 1, ffi::FALSE, matrix.to_cols_array().as_ptr()) }
}

unsafe fn set_vec3(gl: &Gles2, location: i32, [x, y, z]: [f32; 3]) {
    unsafe { gl.Uniform3f(location, x, y, z) }
}

/// Undo the state changes Smithay's renderer does not make for itself.
///
/// Smithay sets the framebuffer, viewport, blending, scissor, program, active
/// texture and vertex attributes itself before it draws. It never touches
/// depth testing, the depth mask or vertex array objects, so it relies on
/// those being at their defaults.
unsafe fn restore_smithay_state(gl: &Gles2) {
    unsafe {
        gl.Disable(ffi::DEPTH_TEST);
        gl.DepthMask(ffi::TRUE);
        gl.BindVertexArray(0);
        gl.UseProgram(0);
        gl.BindTexture(ffi::TEXTURE_2D, 0);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
    }
}

impl Target {
    fn new(renderer: &mut GlesRenderer, size: Size<i32, Physical>) -> Result<Self, Box<dyn Error>> {
        let (framebuffer, depth, color) = renderer.with_context(|gl| unsafe {
            let mut color = 0;
            gl.GenTextures(1, &mut color);
            gl.BindTexture(ffi::TEXTURE_2D, color);
            gl.TexImage2D(
                ffi::TEXTURE_2D,
                0,
                ffi::RGBA8 as i32,
                size.w,
                size.h,
                0,
                ffi::RGBA,
                ffi::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            for (parameter, value) in [
                (ffi::TEXTURE_MIN_FILTER, ffi::LINEAR),
                (ffi::TEXTURE_MAG_FILTER, ffi::LINEAR),
                (ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE),
                (ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE),
            ] {
                gl.TexParameteri(ffi::TEXTURE_2D, parameter, value as i32);
            }
            gl.BindTexture(ffi::TEXTURE_2D, 0);

            let mut depth = 0;
            gl.GenRenderbuffers(1, &mut depth);
            gl.BindRenderbuffer(ffi::RENDERBUFFER, depth);
            gl.RenderbufferStorage(ffi::RENDERBUFFER, ffi::DEPTH_COMPONENT24, size.w, size.h);
            gl.BindRenderbuffer(ffi::RENDERBUFFER, 0);

            let mut framebuffer = 0;
            gl.GenFramebuffers(1, &mut framebuffer);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, framebuffer);
            gl.FramebufferTexture2D(
                ffi::FRAMEBUFFER,
                ffi::COLOR_ATTACHMENT0,
                ffi::TEXTURE_2D,
                color,
                0,
            );
            gl.FramebufferRenderbuffer(
                ffi::FRAMEBUFFER,
                ffi::DEPTH_ATTACHMENT,
                ffi::RENDERBUFFER,
                depth,
            );
            let status = gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);

            if status != ffi::FRAMEBUFFER_COMPLETE {
                gl.DeleteFramebuffers(1, &framebuffer);
                gl.DeleteRenderbuffers(1, &depth);
                gl.DeleteTextures(1, &color);
                return Err(format!("world framebuffer incomplete: status {status:#x}"));
            }
            Ok((framebuffer, depth, color))
        })??;

        // Safety: `color` is a valid, upright, non-external 2D texture that
        // nothing else will delete.
        let color = unsafe {
            GlesTexture::from_raw(
                renderer,
                Some(ffi::RGBA8),
                true,
                color,
                (size.w, size.h).into(),
            )
        };

        Ok(Self {
            size,
            framebuffer,
            depth,
            color,
        })
    }

    fn destroy(self, renderer: &mut GlesRenderer) -> Result<(), Box<dyn Error>> {
        renderer.with_context(|gl| unsafe {
            gl.DeleteFramebuffers(1, &self.framebuffer);
            gl.DeleteRenderbuffers(1, &self.depth);
        })?;
        // Dropping `self.color` hands the texture back to Smithay to delete.
        Ok(())
    }
}
