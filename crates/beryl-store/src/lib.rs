//! Store abstraction, standalone LocalStore, DaemonStore, binary cache client, profiles, and GC.

pub mod cache;
pub mod daemon;
pub mod error;
pub mod gc;
pub mod local;
pub mod profile;
pub mod store;

pub use cache::{BinaryCacheClient, BinaryCacheConfig, FetchedNarInfo};
pub use daemon::{DEFAULT_DAEMON_SOCKET, DaemonStore};
pub use error::{Result, StoreError};
pub use gc::{GcReport, collect_garbage, collect_gc_roots};
pub use local::LocalStore;
pub use profile::ProfileManager;
pub use store::{Store, ValidPathInfo};
