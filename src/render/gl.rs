//! Small helpers over the raw GL bindings.

use std::{
    error::Error,
    ffi::{CStr, c_char},
};

use smithay::backend::renderer::gles::ffi::{self, Gles2, types::GLenum};

/// Read one of the `glGetString` values.
///
/// # Safety
/// The context `gl` belongs to must be current.
pub unsafe fn get_string(gl: &Gles2, name: GLenum) -> String {
    let ptr = unsafe { gl.GetString(name) } as *const c_char;
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Compile and link a shader program.
///
/// # Safety
/// The context `gl` belongs to must be current.
pub unsafe fn link_program(
    gl: &Gles2,
    vertex_src: &str,
    fragment_src: &str,
) -> Result<u32, Box<dyn Error>> {
    unsafe {
        let vertex = compile_shader(gl, ffi::VERTEX_SHADER, vertex_src)?;
        let fragment = compile_shader(gl, ffi::FRAGMENT_SHADER, fragment_src)
            .inspect_err(|_| gl.DeleteShader(vertex))?;

        let program = gl.CreateProgram();
        gl.AttachShader(program, vertex);
        gl.AttachShader(program, fragment);
        gl.LinkProgram(program);
        // Once linked, the program no longer needs the shader objects.
        gl.DeleteShader(vertex);
        gl.DeleteShader(fragment);

        let mut linked = 0;
        gl.GetProgramiv(program, ffi::LINK_STATUS, &mut linked);
        if linked == 0 {
            let mut log = vec![0u8; 1024];
            let mut len = 0;
            gl.GetProgramInfoLog(program, log.len() as i32, &mut len, log.as_mut_ptr().cast());
            gl.DeleteProgram(program);
            let log = String::from_utf8_lossy(&log[..len.max(0) as usize]).into_owned();
            return Err(format!("shader program failed to link: {log}").into());
        }
        Ok(program)
    }
}

unsafe fn compile_shader(gl: &Gles2, kind: GLenum, source: &str) -> Result<u32, Box<dyn Error>> {
    unsafe {
        let shader = gl.CreateShader(kind);
        let ptr = source.as_ptr().cast::<c_char>();
        let len = source.len() as i32;
        gl.ShaderSource(shader, 1, &ptr, &len);
        gl.CompileShader(shader);

        let mut compiled = 0;
        gl.GetShaderiv(shader, ffi::COMPILE_STATUS, &mut compiled);
        if compiled == 0 {
            let mut log = vec![0u8; 1024];
            let mut len = 0;
            gl.GetShaderInfoLog(shader, log.len() as i32, &mut len, log.as_mut_ptr().cast());
            gl.DeleteShader(shader);
            let log = String::from_utf8_lossy(&log[..len.max(0) as usize]).into_owned();
            return Err(format!("shader failed to compile: {log}").into());
        }
        Ok(shader)
    }
}
