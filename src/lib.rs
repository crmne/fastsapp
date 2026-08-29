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
pub mod paths;
pub mod qr;
pub mod settings;
pub mod system_fonts;
pub mod theme;
pub mod ui;
pub mod util;
