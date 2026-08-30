//! Fastsapp's internals, exposed so diagnostics and tests can reach them.

pub mod animation;
pub mod app;
pub mod archive;
pub mod backend;
#[cfg(any(test, feature = "demo"))]
pub mod demo;
pub mod emoji;
pub mod markup;
pub mod model;
pub mod notify;
pub mod paths;
pub mod qr;
pub mod settings;
pub mod single_instance;
pub mod system_fonts;
pub mod theme;
#[cfg(target_os = "linux")]
pub mod tray;
#[cfg(not(target_os = "linux"))]
#[path = "tray_native.rs"]
pub mod tray;
pub mod ui;
pub mod util;
