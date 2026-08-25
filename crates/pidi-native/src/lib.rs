//! Native jambox kiosk UI: 800×480 SDL/GLES client for jambox-engine.
//!
//! Musical time stays in the engine. This process may drop frames; it must
//! never own a note release.

pub mod client;
pub mod font;
#[cfg(feature = "sdl")]
pub mod gles;
pub mod input;
pub mod layout;
pub mod mode;
pub mod model;
pub mod phrases;
pub mod render;
pub mod scene;
pub mod seq;
#[cfg(feature = "sdl")]
pub mod sdl_backend;

pub use client::NativeClient;
pub use input::TouchEvent;
pub use layout::{Hit, Layout, Surface};
pub use mode::UiMode;
pub use model::{NativeModel, RepeatDivisionChoice};
pub use render::{Frame, SCREEN_H, SCREEN_W};
