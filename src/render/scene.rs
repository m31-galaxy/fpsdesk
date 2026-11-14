//! Pass 2: draw the 3D world into a framebuffer of our own.
//!
//! This is the only place that issues raw GL draw calls. The context is shared
//! with Smithay's renderer, whose GL state is an implementation detail we must
//! not disturb, so `render` ends by putting back everything it changed that
//! Smithay does not set for itself (see `restore_smithay_state`).

use std::error::Error;

use glam::Mat4;
use smithay::{
    backend::renderer::gles::{
        GlesRenderer, GlesTexture,
        ffi::{self, Gles2},
    },
    utils::{Physical, Size},
};

use super::gl;

const VERTEX_SHADER: &str = "#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
uniform mat4 u_mvp;
out vec2 v_uv;
void main() {
    v_uv = a_uv;
    gl_Position = u_mvp * vec4(a_position, 0.0, 1.0);
}
";

const FRAGMENT_SHADER: &str = "#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_texture;
out vec4 color;
void main() {
    color = texture(u_texture, v_uv);
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
}

pub struct SceneRenderer {
    program: u32,
    u_mvp: i32,
    u_texture: i32,
    vao: u32,
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

            let program = gl::link_program(gl, VERTEX_SHADER, FRAGMENT_SHADER)?;
            let u_mvp = gl.GetUniformLocation(program, c"u_mvp".as_ptr());
            let u_texture = gl.GetUniformLocation(program, c"u_texture".as_ptr());

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

            Ok(Self {
                program,
                u_mvp,
                u_texture,
                vao,
                target: None,
            })
        })?
    }

    /// Draw `quads`, already ordered back to front, as seen through
    /// `view_projection`. Returns the texture holding the finished image; its
    /// first row is the bottom of the picture, as usual for GL framebuffers.
    pub fn render(
        &mut self,
        renderer: &mut GlesRenderer,
        size: Size<i32, Physical>,
        view_projection: Mat4,
        sky: [f32; 3],
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

        renderer.with_context(|gl| unsafe {
            gl.BindFramebuffer(ffi::FRAMEBUFFER, target.framebuffer);
            gl.Viewport(0, 0, size.w, size.h);
            gl.Disable(ffi::SCISSOR_TEST);
            gl.Enable(ffi::DEPTH_TEST);
            gl.ClearColor(sky[0], sky[1], sky[2], 1.0);
            gl.Clear(ffi::COLOR_BUFFER_BIT | ffi::DEPTH_BUFFER_BIT);

            // Window textures are premultiplied.
            gl.Enable(ffi::BLEND);
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);

            gl.UseProgram(self.program);
            gl.BindVertexArray(self.vao);
            gl.ActiveTexture(ffi::TEXTURE0);
            gl.Uniform1i(self.u_texture, 0);
            for quad in quads {
                let mvp = (view_projection * quad.model).to_cols_array();
                gl.UniformMatrix4fv(self.u_mvp, 1, ffi::FALSE, mvp.as_ptr());
                gl.BindTexture(ffi::TEXTURE_2D, quad.texture);
                gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
            }

            restore_smithay_state(gl);
        })?;

        Ok(target.color.clone())
    }
}

/// Undo the state changes Smithay's renderer does not make for itself.
///
/// Smithay sets the framebuffer, viewport, blending, scissor, program, active
/// texture and vertex attributes itself before it draws. It never touches
/// depth testing or vertex array objects, so it relies on those being off.
unsafe fn restore_smithay_state(gl: &Gles2) {
    unsafe {
        gl.Disable(ffi::DEPTH_TEST);
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
