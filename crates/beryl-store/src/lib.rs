//! Store abstraction, standalone LocalStore, binary cache client, profiles, and GC.

pub mod error;
pub mod local;
pub mod store;

pub use error::{Result, StoreError};
pub use local::LocalStore;
pub use store::{Store, ValidPathInfo};
