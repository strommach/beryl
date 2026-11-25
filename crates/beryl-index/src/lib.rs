//! Three-tier `nixpkgs` resolution (`beryl.lock` -> upstream Hydra index -> `nix eval` fallback).

pub mod error;
pub mod lockfile;
pub mod resolver;

pub use error::{IndexError, Result};
pub use lockfile::{LockedPackage, Lockfile, NixpkgsPin, ResolutionSource};
pub use resolver::{HydraConfig, NixpkgsResolver, current_system};
