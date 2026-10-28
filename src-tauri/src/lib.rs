#[cfg(feature = "desktop")]
mod app;
pub mod download;
pub mod network;
pub mod sources;
#[cfg(feature = "desktop")]
pub use app::run;
