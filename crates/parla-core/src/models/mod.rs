//! Model catalogue (`manifest.json`) and the resumable, hash-verified downloader.
pub mod download;
pub mod manifest;

pub use download::{install_item, installed_dir, is_installed, uninstall_item, Cancel};
pub use manifest::{FileSpec, Item, Manifest, Slot};
