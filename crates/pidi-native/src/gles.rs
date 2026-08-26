//! OpenGL ES 2.0 presenter: one color-quad mesh and one glyph mesh per frame.

use glow::{HasContext, NativeBuffer, NativeProgram, NativeTexture, NativeUniformLocation};

use crate::font::{self, FontStyle};
use crate::scene::{ColorQuad, GlyphQuad, Scene};

const SCREEN_W: f32 = 800.0;
const SCREEN_H: f32 = 480.0;

const COLOR_VS_ES: &str = r#"
#version 100
attribute vec2 a_pos;
attribute vec4 a_color;
uniform vec2 u_screen;
varying vec4 v_color;
void main() {
    vec2 ndc = vec2((a_pos.x / u_screen.x) * 2.0 - 1.0, 1.0 - (a_pos.y / u_screen.y) * 2.0);
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_color = a_color;
}
"#;

const COLOR_FS_ES: &str = r#"
#version 100
precision mediump float;
varying vec4 v_color;
void main() { gl_FragColor = v_color; }
"#;

const TEX_VS_ES: &str = r#"
#version 100
attribute vec2 a_pos;
attribute vec2 a_uv;
attribute vec4 a_color;
uniform vec2 u_screen;
varying vec2 v_uv;
varying vec4 v_color;
void main() {
    vec2 ndc = vec2((a_pos.x / u_screen.x) * 2.0 - 1.0, 1.0 - (a_pos.y / u_screen.y) * 2.0);
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_uv = a_uv;
    v_color = a_color;
}
"#;

const TEX_FS_ES: &str = r#"
#version 100
precision mediump float;
varying vec2 v_uv;
varying vec4 v_color;
uniform sampler2D u_tex;
void main() {
    float a = texture2D(u_tex, v_uv).a;
    gl_FragColor = vec4(v_color.rgb, v_color.a * a);
}
"#;

const COLOR_VS_120: &str = r#"
#version 120
attribute vec2 a_pos;
attribute vec4 a_color;
uniform vec2 u_screen;
varying vec4 v_color;
void main() {
    vec2 ndc = vec2((a_pos.x / u_screen.x) * 2.0 - 1.0, 1.0 - (a_pos.y / u_screen.y) * 2.0);
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_color = a_color;
}
"#;

const COLOR_FS_120: &str = r#"
#version 120
varying vec4 v_color;
void main() { gl_FragColor = v_color; }
"#;

const TEX_VS_120: &str = r#"
#version 120
attribute vec2 a_pos;
attribute vec2 a_uv;
attribute vec4 a_color;
uniform vec2 u_screen;
varying vec2 v_uv;
varying vec4 v_color;
void main() {
    vec2 ndc = vec2((a_pos.x / u_screen.x) * 2.0 - 1.0, 1.0 - (a_pos.y / u_screen.y) * 2.0);
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_uv = a_uv;
    v_color = a_color;
}
"#;

const TEX_FS_120: &str = r#"
#version 120
varying vec2 v_uv;
varying vec4 v_color;
uniform sampler2D u_tex;
void main() {
    float a = texture2D(u_tex, v_uv).a;
    gl_FragColor = vec4(v_color.rgb, v_color.a * a);
}
"#;

pub struct Renderer {
    color_prog: NativeProgram,
    tex_prog: NativeProgram,
    color_vbo: NativeBuffer,
    tex_vbo: NativeBuffer,
    atlas_retro: NativeTexture,
    atlas_smooth: NativeTexture,
    color_screen: NativeUniformLocation,
    tex_screen: NativeUniformLocation,
    tex_sampler: NativeUniformLocation,
    color_pos: u32,
    color_col: u32,
    tex_pos: u32,
    tex_uv: u32,
    tex_col: u32,
}

impl Renderer {
    /// Compile programs, upload the glyph atlas, allocate stream VBOs.
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let color_prog = compile_with_fallback(
                gl,
                COLOR_VS_ES,
                COLOR_FS_ES,
                COLOR_VS_120,
                COLOR_FS_120,
            )?;
            let tex_prog =
                compile_with_fallback(gl, TEX_VS_ES, TEX_FS_ES, TEX_VS_120, TEX_FS_120)?;

            let color_pos = attrib(gl, color_prog, "a_pos")?;
            let color_col = attrib(gl, color_prog, "a_color")?;
            let color_screen = uniform(gl, color_prog, "u_screen")?;

            let tex_pos = attrib(gl, tex_prog, "a_pos")?;
            let tex_uv = attrib(gl, tex_prog, "a_uv")?;
            let tex_col = attrib(gl, tex_prog, "a_color")?;
            let tex_screen = uniform(gl, tex_prog, "u_screen")?;
            let tex_sampler = uniform(gl, tex_prog, "u_tex")?;

            let color_vbo = gl.create_buffer().map_err(|e| e.to_string())?;
            let tex_vbo = gl.create_buffer().map_err(|e| e.to_string())?;

            let (aw, ah, pixels) = font::atlas_rgba();
            let atlas_retro = upload_atlas(gl, aw, ah, &pixels, false)?;
            let (sw, sh, smooth_pixels) = font::atlas_rgba_for(FontStyle::Smooth);
            let atlas_smooth = upload_atlas(gl, sw, sh, &smooth_pixels, true)?;

            Ok(Self {
                color_prog,
                tex_prog,
                color_vbo,
                tex_vbo,
                atlas_retro,
                atlas_smooth,
                color_screen,
                tex_screen,
                tex_sampler,
                color_pos,
                color_col,
                tex_pos,
                tex_uv,
                tex_col,
            })
        }
    }

    /// Draw the scene with two `glDrawArrays` calls (color quads, then glyphs).
    pub unsafe fn draw(&mut self, gl: &glow::Context, scene: &Scene) {
        let [cr, cg, cb, _] = unpack_rgb(scene.clear);
        gl.viewport(0, 0, 800, 480);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.clear_color(cr, cg, cb, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);

        let mut color_verts = Vec::with_capacity(scene.color.len() * 6 * 6);
        for q in &scene.color {
            push_color_quad(&mut color_verts, q);
        }
        gl.use_program(Some(self.color_prog));
        gl.uniform_2_f32(Some(&self.color_screen), SCREEN_W, SCREEN_H);
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.color_vbo));
        upload_f32(gl, &color_verts);
        let color_stride = 6 * 4;
        gl.enable_vertex_attrib_array(self.color_pos);
        gl.vertex_attrib_pointer_f32(self.color_pos, 2, glow::FLOAT, false, color_stride, 0);
        gl.enable_vertex_attrib_array(self.color_col);
        gl.vertex_attrib_pointer_f32(self.color_col, 4, glow::FLOAT, false, color_stride, 8);
        gl.draw_arrays(glow::TRIANGLES, 0, (color_verts.len() / 6) as i32);
        gl.disable_vertex_attrib_array(self.color_pos);
        gl.disable_vertex_attrib_array(self.color_col);

        let mut tex_verts = Vec::with_capacity(scene.glyphs.len() * 6 * 8);
        for q in &scene.glyphs {
            push_glyph_quad(&mut tex_verts, q);
        }
        gl.enable(glow::BLEND);
        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        gl.use_program(Some(self.tex_prog));
        gl.uniform_2_f32(Some(&self.tex_screen), SCREEN_W, SCREEN_H);
        gl.uniform_1_i32(Some(&self.tex_sampler), 0);
        gl.active_texture(glow::TEXTURE0);
        let style = scene.font_style.resolved();
        let (atlas, linear) = match style {
            FontStyle::Smooth => (self.atlas_smooth, true),
            FontStyle::Retro => (self.atlas_retro, false),
        };
        gl.bind_texture(glow::TEXTURE_2D, Some(atlas));
        let filter = if linear {
            glow::LINEAR as i32
        } else {
            glow::NEAREST as i32
        };
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.tex_vbo));
        upload_f32(gl, &tex_verts);
        let tex_stride = 8 * 4;
        gl.enable_vertex_attrib_array(self.tex_pos);
        gl.vertex_attrib_pointer_f32(self.tex_pos, 2, glow::FLOAT, false, tex_stride, 0);
        gl.enable_vertex_attrib_array(self.tex_uv);
        gl.vertex_attrib_pointer_f32(self.tex_uv, 2, glow::FLOAT, false, tex_stride, 8);
        gl.enable_vertex_attrib_array(self.tex_col);
        gl.vertex_attrib_pointer_f32(self.tex_col, 4, glow::FLOAT, false, tex_stride, 16);
        gl.draw_arrays(glow::TRIANGLES, 0, (tex_verts.len() / 8) as i32);
        gl.disable_vertex_attrib_array(self.tex_pos);
        gl.disable_vertex_attrib_array(self.tex_uv);
        gl.disable_vertex_attrib_array(self.tex_col);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.disable(glow::BLEND);
        gl.use_program(None);
    }
}

unsafe fn upload_atlas(
    gl: &glow::Context,
    aw: u32,
    ah: u32,
    pixels: &[u8],
    linear: bool,
) -> Result<NativeTexture, String> {
    let atlas = gl.create_texture().map_err(|e| e.to_string())?;
    gl.bind_texture(glow::TEXTURE_2D, Some(atlas));
    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA as i32,
        aw as i32,
        ah as i32,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(Some(pixels)),
    );
    let filter = if linear {
        glow::LINEAR as i32
    } else {
        glow::NEAREST as i32
    };
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_WRAP_S,
        glow::CLAMP_TO_EDGE as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_WRAP_T,
        glow::CLAMP_TO_EDGE as i32,
    );
    gl.bind_texture(glow::TEXTURE_2D, None);
    Ok(atlas)
}

unsafe fn attrib(gl: &glow::Context, program: NativeProgram, name: &str) -> Result<u32, String> {
    gl.get_attrib_location(program, name)
        .ok_or_else(|| format!("{name} missing"))
}

unsafe fn uniform(
    gl: &glow::Context,
    program: NativeProgram,
    name: &str,
) -> Result<NativeUniformLocation, String> {
    gl.get_uniform_location(program, name)
        .ok_or_else(|| format!("{name} missing"))
}

unsafe fn upload_f32(gl: &glow::Context, verts: &[f32]) {
    let bytes = std::slice::from_raw_parts(verts.as_ptr() as *const u8, verts.len() * 4);
    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
}

fn unpack_rgb(color: u32) -> [f32; 4] {
    crate::scene::unpack_rgb(color)
}

fn push_color_quad(out: &mut Vec<f32>, q: &ColorQuad) {
    let x0 = q.x;
    let y0 = q.y;
    let x1 = q.x + q.w;
    let y1 = q.y + q.h;
    let [r, g, b, a] = unpack_rgb(q.color);
    out.extend_from_slice(&[
        x0, y0, r, g, b, a, x1, y0, r, g, b, a, x1, y1, r, g, b, a, x0, y0, r, g, b, a, x1, y1, r,
        g, b, a, x0, y1, r, g, b, a,
    ]);
}

fn push_glyph_quad(out: &mut Vec<f32>, q: &GlyphQuad) {
    let x0 = q.x;
    let y0 = q.y;
    let x1 = q.x + q.w;
    let y1 = q.y + q.h;
    let [r, g, b, a] = unpack_rgb(q.color);
    let (u0, v0, u1, v1) = (q.u0, q.v0, q.u1, q.v1);
    out.extend_from_slice(&[
        x0, y0, u0, v0, r, g, b, a, x1, y0, u1, v0, r, g, b, a, x1, y1, u1, v1, r, g, b, a, x0, y0,
        u0, v0, r, g, b, a, x1, y1, u1, v1, r, g, b, a, x0, y1, u0, v1, r, g, b, a,
    ]);
}

unsafe fn compile_with_fallback(
    gl: &glow::Context,
    vs_es: &str,
    fs_es: &str,
    vs_120: &str,
    fs_120: &str,
) -> Result<NativeProgram, String> {
    match compile(gl, vs_es, fs_es) {
        Ok(p) => Ok(p),
        Err(first) => compile(gl, vs_120, fs_120)
            .map_err(|second| format!("ES shaders: {first}; GL 1.20 shaders: {second}")),
    }
}

unsafe fn compile(gl: &glow::Context, vs_src: &str, fs_src: &str) -> Result<NativeProgram, String> {
    let program = gl.create_program().map_err(|e| e.to_string())?;
    let vs = gl
        .create_shader(glow::VERTEX_SHADER)
        .map_err(|e| e.to_string())?;
    gl.shader_source(vs, vs_src);
    gl.compile_shader(vs);
    if !gl.get_shader_compile_status(vs) {
        let log = gl.get_shader_info_log(vs);
        gl.delete_shader(vs);
        gl.delete_program(program);
        return Err(format!("vertex: {log}"));
    }
    let fs = gl
        .create_shader(glow::FRAGMENT_SHADER)
        .map_err(|e| e.to_string())?;
    gl.shader_source(fs, fs_src);
    gl.compile_shader(fs);
    if !gl.get_shader_compile_status(fs) {
        let log = gl.get_shader_info_log(fs);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        gl.delete_program(program);
        return Err(format!("fragment: {log}"));
    }
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        gl.delete_program(program);
        return Err(format!("link: {log}"));
    }
    gl.detach_shader(program, vs);
    gl.detach_shader(program, fs);
    gl.delete_shader(vs);
    gl.delete_shader(fs);
    Ok(program)
}
