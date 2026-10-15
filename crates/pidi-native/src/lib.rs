//! Native jambox kiosk UI: 800×480 SDL/GLES client for jambox-engine.
//!
//! Musical time stays in the engine. This process may drop frames; it must
//! never own a note release.

pub mod arp;
pub mod chords;
pub mod client;
pub mod font;
#[cfg(feature = "sdl")]
pub mod gles;
pub mod host;
pub mod input;
pub mod kaoss_ui;
pub mod kaoss_viz;
pub mod layout;
pub mod mode;
pub mod model;
pub mod ota;
pub mod paths;
pub mod phrases;
pub mod piano_roll;
pub mod presets;
pub mod probe;
pub mod render;
pub mod scene;
pub mod screensaver;
pub mod scroll;
#[cfg(feature = "sdl")]
pub mod sdl_backend;
pub mod seq;
pub mod session;
pub mod smooth_font;
pub mod song_viz;
pub mod songs;
pub mod throttle;
pub mod voice_bake;
pub mod wave_viz;
pub mod waves;
pub mod wifi;
pub mod wifi_power;

pub use client::NativeClient;
pub use font::FontStyle;
pub use input::TouchEvent;
pub use layout::{Hit, Layout, Surface};
pub use mode::UiMode;
pub use model::{NativeModel, RepeatDivisionChoice};
pub use render::{Frame, SCREEN_H, SCREEN_W};
pub use session::{KaossFxTarget, OutMode};
