//! APFSReader core: read-only access to HFS+ / APFS (unencrypted) volumes.

pub mod aligned;
pub mod apfs;
pub mod decmpfs;
pub mod device;
pub mod error;
pub mod gpt;
pub mod hfsplus;
pub mod image;
pub mod lzvn;
pub mod partition;
#[cfg(windows)]
pub mod physical;
pub mod remote;
pub mod udif;
mod util;
pub mod vfs;
pub mod winnames;

pub use error::{Error, Result};
