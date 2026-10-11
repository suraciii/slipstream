mod decoded;
pub mod jpeg;
pub mod tiff;
pub use decoded::{ImageFacts, inspect_image, validate};
