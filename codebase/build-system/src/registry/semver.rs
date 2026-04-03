//! Semver version resolution for registry packages
use semver::{Version, VersionReq};

/// Parse a version requirement string
pub fn parse_version_req(req: &str) -> Result<VersionReq, String> {
    VersionReq::parse(req).map_err(|e| format!("Invalid version requirement: {}", e))
}

/// Find the best matching version from available versions
pub fn resolve_version(available: &[Version], req: &VersionReq) -> Option<Version> {
    available.iter().filter(|v| req.matches(v)).max().cloned()
}

/// Parse a version string
pub fn parse_version(version: &str) -> Result<Version, String> {
    Version::parse(version).map_err(|e| format!("Invalid version: {}", e))
}

/// Convert version to string for lockfile
pub fn version_to_string(version: &Version) -> String {
    version.to_string()
}

/// Get the latest version from available versions
pub fn latest_version(available: &[Version]) -> Option<Version> {
    available.iter().max().cloned()
}

/// Filter valid semver versions from a list of strings
/// Strips 'v' prefix from tags (e.g., "v1.0.0" -> "1.0.0")
pub fn filter_valid_versions(tags: &[String]) -> Vec<Version> {
    tags.iter()
        .filter_map(|t| {
            let v_str = t.strip_prefix('v').unwrap_or(t);
            Version::parse(v_str).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_req() {
        assert!(parse_version_req("^1.0.0").is_ok());
        assert!(parse_version_req(">=1.0.0").is_ok());
        assert!(parse_version_req("1.0.0").is_ok());
        assert!(parse_version_req("invalid").is_err());
    }

    #[test]
    fn test_resolve_version() {
        let versions = vec![
            Version::parse("1.0.0").unwrap(),
            Version::parse("1.2.0").unwrap(),
            Version::parse("2.0.0").unwrap(),
        ];
        let req = VersionReq::parse("^1.0.0").unwrap();
        let resolved = resolve_version(&versions, &req);
        assert_eq!(resolved, Some(Version::parse("1.2.0").unwrap()));
    }

    #[test]
    fn test_resolve_version_no_match() {
        let versions = vec![
            Version::parse("1.0.0").unwrap(),
            Version::parse("1.2.0").unwrap(),
        ];
        let req = VersionReq::parse("^2.0.0").unwrap();
        let resolved = resolve_version(&versions, &req);
        assert_eq!(resolved, None);
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(
            parse_version("1.2.3").unwrap(),
            Version::parse("1.2.3").unwrap()
        );
        assert!(parse_version("not-a-version").is_err());
    }

    #[test]
    fn test_latest_version() {
        let versions = vec![
            Version::parse("1.0.0").unwrap(),
            Version::parse("2.0.0").unwrap(),
            Version::parse("1.5.0").unwrap(),
        ];
        assert_eq!(
            latest_version(&versions),
            Some(Version::parse("2.0.0").unwrap())
        );
    }

    #[test]
    fn test_filter_valid_versions() {
        let tags = vec![
            "v1.0.0".to_string(),
            "1.2.0".to_string(),
            "invalid".to_string(),
            "2.0.0".to_string(),
        ];
        let valid = filter_valid_versions(&tags);
        assert_eq!(valid.len(), 3);
        assert!(valid.contains(&Version::parse("1.0.0").unwrap()));
        assert!(valid.contains(&Version::parse("1.2.0").unwrap()));
        assert!(valid.contains(&Version::parse("2.0.0").unwrap()));
    }
}
