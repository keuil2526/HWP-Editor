//! hwpedit-core: the file-integrity layer that sits between the document
//! engine (rhwp) and the UI. It owns the original bytes and decides whether,
//! how and where anything is written.

pub mod format;
pub mod save;

pub use format::{detect, Format};
pub use save::{save, save_new, OpenedFile, SaveError, SaveOptions, SaveOutcome};
