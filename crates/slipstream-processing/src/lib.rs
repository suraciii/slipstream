//! Local Photo development contracts of the application container: the
//! native engine execution boundary and the Development TIFF validator.
//! This crate does not resolve Originals, edit Photos, or publish Exports.
mod mcp_client;
mod native_development;
mod photo_tiff;

pub mod local_photo;
pub mod photo_profile;
pub use mcp_client::run_supervisor;
