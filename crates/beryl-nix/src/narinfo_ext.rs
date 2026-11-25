//! Helpers for parsing and verifying `.narinfo` metadata from Nix binary caches.

use nix_compat::narinfo::{NarInfo, VerifyingKey};

use crate::error::{NixCompatError, Result};

/// Official public verifying key for `https://cache.nixos.org`.
pub const CACHE_NIXOS_ORG_PUBLIC_KEY: &str =
    "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";

/// Parse a `name:base64` Nix verifying key string.
pub fn parse_verifying_key(spec: &str) -> Result<VerifyingKey> {
    VerifyingKey::parse(spec).map_err(|e| NixCompatError::InvalidVerifyingKey(e.to_string()))
}

/// Parse the default `cache.nixos.org` verifying key.
pub fn default_nixos_verifying_key() -> VerifyingKey {
    VerifyingKey::parse(CACHE_NIXOS_ORG_PUBLIC_KEY)
        .expect("built-in cache.nixos.org public key must be valid")
}

/// Parse a `.narinfo` document.
pub fn parse_narinfo(input: &str) -> Result<NarInfo<'_>> {
    NarInfo::parse(input).map_err(|e| NixCompatError::InvalidNarInfo(e.to_string()))
}

/// Verify that `narinfo` carries at least one valid Ed25519 signature from `trusted_keys`.
pub fn verify_narinfo(narinfo: &NarInfo<'_>, trusted_keys: &[VerifyingKey]) -> bool {
    let fingerprint = narinfo.fingerprint();
    for sig in &narinfo.signatures {
        for key in trusted_keys {
            if key.verify(&fingerprint, sig) {
                return true;
            }
        }
    }
    false
}
