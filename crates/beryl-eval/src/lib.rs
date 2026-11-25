//! Lua 5.4 evaluator compiling `pkg.lua` definitions into Nix ATerm `.drv` files.

pub mod error;
pub mod evaluator;
pub mod project;
pub mod store_ref;

pub use error::{EvalError, Result};
pub use evaluator::{EvalResult, Evaluator, NixpkgsLookup};
pub use project::{DevShellRef, DevShellSpec, ProjectManifest};
pub use store_ref::StoreRef;
