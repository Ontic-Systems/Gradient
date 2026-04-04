//! Package registry client for fetching remote dependencies

pub mod client;
pub mod github;
pub mod semver;

// Re-export types from the semver crate (not the local module)
// Using :: prefix to refer to the external crate, not the local module
pub use ::semver::Version;

// Re-export clients
pub use github::GitHubClient;
