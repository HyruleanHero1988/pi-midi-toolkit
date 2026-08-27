//! Native jambox kiosk UI: 800×480 SDL/GLES client for jambox-engine.
//!
//! Musical time stays in the engine. This process may drop frames; it must
//! never own a note release.

pub mod client;
pub mod font;
pub mod smooth_font;
#[cfg(feature = "sdl")]
pub mod gles;
pub mod host;
pub mod input;
pub mod kaoss_ui;
pub mod kaoss_viz;
pub mod layout;
pub mod mode;
pub mod chords;
pub mod model;
pub mod phrases;
pub mod presets;
pub mod render;
pub mod scene;
pub mod scroll;
pub mod screensaver;
pub mod seq;
pub mod session;
pub mod songs;
pub mod voice_bake;
pub mod waves;
#[cfg(feature = "sdl")]
pub mod sdl_backend;

pub use client::NativeClient;
pub use input::TouchEvent;
pub use font::FontStyle;
pub use layout::{Hit, Layout, Surface};
pub use mode::UiMode;
pub use model::{NativeModel, RepeatDivisionChoice};
pub use session::OutMode;
pub use render::{Frame, SCREEN_H, SCREEN_W};
