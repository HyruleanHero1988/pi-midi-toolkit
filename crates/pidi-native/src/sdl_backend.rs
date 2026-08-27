//! SDL2 window + OpenGL ES 2 context + FingerId events.
//!
//! On the Pi this is the KMSDRM path (`SDL_VIDEODRIVER=kmsdrm`). On a desktop
//! host it falls back to a windowed OpenGL 2.1 context if GLES 2 is unavailable.
//! SDL audio is forced to the dummy driver so this process never owns ALSA.

use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton;
use sdl2::video::{GLProfile, SwapInterval};
use sdl2::{EventPump, Sdl, VideoSubsystem};

use crate::gles;
use crate::input::{self, TouchEvent};
use crate::render::{SCREEN_H, SCREEN_W};
use crate::scene::Scene;

pub struct SdlDisplay {
    _sdl: Sdl,
    _video: VideoSubsystem,
    window: sdl2::video::Window,
    _gl_ctx: sdl2::video::GLContext,
    gl: glow::Context,
    renderer: gles::Renderer,
    event_pump: EventPump,
    mouse_down: bool,
}

impl SdlDisplay {
    pub fn open(fullscreen: bool) -> Result<Self, String> {
        sdl2::hint::set("SDL_AUDIODRIVER", "dummy");
        let sdl = sdl2::init()?;
        let video: VideoSubsystem = sdl.video()?;

        let (window, gl_ctx) = match open_gl_window(&video, fullscreen, GLProfile::GLES, 2, 0) {
            Ok(pair) => pair,
            Err(gles_err) => {
                eprintln!(
                    "pidi-native: GLES 2.0 context failed ({gles_err}); trying OpenGL 2.1"
                );
                open_gl_window(&video, fullscreen, GLProfile::Compatibility, 2, 1)?
            }
        };
        window.gl_make_current(&gl_ctx)?;
        let _ = video.gl_set_swap_interval(SwapInterval::VSync);

        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                video.gl_get_proc_address(name) as *const _
            })
        };
        let renderer = gles::Renderer::new(&gl)?;
        let event_pump = sdl.event_pump()?;

        // Appliance is touch-first; a visible cursor parks in a corner under KMSDRM.
        if fullscreen {
            sdl.mouse().show_cursor(false);
        }

        eprintln!(
            "pidi-native: SDL/GL presenter {}x{} fullscreen={fullscreen}",
            window.size().0,
            window.size().1
        );

        Ok(Self {
            _sdl: sdl,
            _video: video,
            window,
            _gl_ctx: gl_ctx,
            gl,
            renderer,
            event_pump,
            mouse_down: false,
        })
    }

    pub fn present(&mut self, scene: &Scene) -> Result<(), String> {
        unsafe {
            self.renderer.draw(&self.gl, scene);
        }
        self.window.gl_swap_window();
        Ok(())
    }

    /// Pump SDL events. Always call this when a window exists, even if touch
    /// comes from evdev — otherwise the compositor/KMS path stalls.
    ///
    /// Returns `false` on quit.
    pub fn poll(&mut self, collect_touch: bool, out: &mut Vec<TouchEvent>) -> bool {
        let (ww, wh) = self.window.size();
        for event in self.event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => return false,
                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => return false,
                Event::FingerDown {
                    finger_id, x, y, ..
                } if collect_touch => {
                    let (px, py) = input::norm_to_px(x, y, SCREEN_W as i32, SCREEN_H as i32);
                    out.push(TouchEvent::Down {
                        id: finger_id as i32,
                        x: px,
                        y: py,
                    });
                }
                Event::FingerMotion {
                    finger_id, x, y, ..
                } if collect_touch => {
                    let (px, py) = input::norm_to_px(x, y, SCREEN_W as i32, SCREEN_H as i32);
                    out.push(TouchEvent::Move {
                        id: finger_id as i32,
                        x: px,
                        y: py,
                    });
                }
                Event::FingerUp { finger_id, .. } if collect_touch => {
                    out.push(TouchEvent::Up {
                        id: finger_id as i32,
                    });
                }
                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } if collect_touch => {
                    self.mouse_down = true;
                    let (px, py) = window_to_surface(x, y, ww, wh);
                    out.push(TouchEvent::Down {
                        id: 0,
                        x: px,
                        y: py,
                    });
                }
                Event::MouseMotion { x, y, .. } if collect_touch && self.mouse_down => {
                    let (px, py) = window_to_surface(x, y, ww, wh);
                    out.push(TouchEvent::Move {
                        id: 0,
                        x: px,
                        y: py,
                    });
                }
                Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } if collect_touch => {
                    self.mouse_down = false;
                    let (px, py) = window_to_surface(x, y, ww, wh);
                    out.push(TouchEvent::Up { id: 0 });
                    let _ = (px, py);
                }
                _ => {}
            }
        }
        true
    }
}

fn window_to_surface(x: i32, y: i32, ww: u32, wh: u32) -> (i32, i32) {
    let sx = (x as i64 * SCREEN_W as i64 / ww.max(1) as i64).clamp(0, SCREEN_W as i64 - 1) as i32;
    let sy = (y as i64 * SCREEN_H as i64 / wh.max(1) as i64).clamp(0, SCREEN_H as i64 - 1) as i32;
    (sx, sy)
}

fn open_gl_window(
    video: &VideoSubsystem,
    fullscreen: bool,
    profile: GLProfile,
    major: u8,
    minor: u8,
) -> Result<(sdl2::video::Window, sdl2::video::GLContext), String> {
    let gl_attr = video.gl_attr();
    gl_attr.set_context_profile(profile);
    gl_attr.set_context_major_version(major);
    gl_attr.set_context_minor_version(minor);
    gl_attr.set_double_buffer(true);
    gl_attr.set_depth_size(0);

    let mut builder = video.window("pidi-native", SCREEN_W as u32, SCREEN_H as u32);
    builder.opengl();
    if fullscreen {
        builder.fullscreen();
    } else {
        builder.position_centered();
    }
    let window = builder.build().map_err(|e| e.to_string())?;
    let ctx = window.gl_create_context().map_err(|e| {
        format!("gl_create_context ({profile:?} {major}.{minor}): {e}")
    })?;
    Ok((window, ctx))
}
