//! The imager application. One binary serves both the desktop interface
//! and headless use; this library holds the logic so it can be tested.

pub mod cli;
#[cfg(feature = "gui")]
mod gui;
pub mod state;
