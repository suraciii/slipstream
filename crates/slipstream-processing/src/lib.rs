//! Local processing-module contracts and supervised native execution.
//! This crate does not resolve Originals, edit Photos, or publish Exports.
pub mod local_film;
pub mod local_photo;
pub mod local_preview;
mod mcp_client;
pub mod modules;
mod native_development;
mod photo_jpeg;
pub mod photo_profile;
mod photo_tiff;

pub use mcp_client::run_supervisor;
