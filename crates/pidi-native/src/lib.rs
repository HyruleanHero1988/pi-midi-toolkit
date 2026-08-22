//! Native vertical-slice UI: one 800×480 KAOSS + drum performance surface.
//!
//! The binary talks to `jambox-engine` over the versioned JSON protocol. Musical
//! time stays in the engine. This process may drop frames; it must never own a
//! note release.

pub mod client;
pub mod font;
#[cfg(feature = "sdl")]
pub mod gles;
pub mod input;
pub mod layout;
pub mod model;
pub mod render;
pub mod scene;
#[cfg(feature = "sdl")]
pub mod sdl_backend;

pub use client::NativeClient;
pub use input::TouchEvent;
pub use layout::{Hit, Layout, Surface};
pub use model::{NativeModel, RepeatDivisionChoice};
pub use render::{Frame, SCREEN_H, SCREEN_W};
