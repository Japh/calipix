//! Image processing algorithms.
//!
//! Functions here work on plain buffers and know nothing about commands,
//! workspaces or user interfaces. Results are intended to match ImageJ 1.x;
//! each function documents the ImageJ behaviour it reproduces.

mod invert;

pub use invert::invert_plane;
