//! Semantic versioning for skill dependencies
//!
//! Supports parsing, comparing, and constraint matching for versions
//! like `1.2.3`, `^1.0.0`, `~1.2.0`, `>=1.0.0`, etc.

use std::fmt;
use std::str::FromStr;

/// A parsed semantic version (major.minor.patch)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Create a new version
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self { major, minor, patch }
    }

    /// Parse from string like "1.2.3"
    pub fn parse(s: &str) -> crate::Result<Self> {
        s.parse()
    }
}

impl FromStr for Version {
    type Err = crate::error::MantaError;

    fn from_str(s: &str) -> crate::Result<Self> {
        let trimmed = s.trim_start_matches('v').trim_start_matches('V');
        let parts: Vec<&str> = trimmed.split('.').collect();

        if parts.len() != 3 {
            return Err(crate::error::MantaError::Validation(format!(
                "Invalid version '{}': expected major.minor.patch",
                s
            )));
        }

        let major = parts[0]
            .parse::<u64>()
            .map_err(|_| crate::error::MantaError::Validation(format!(
                "Invalid major version in '{}'",
                s
            )))?;
        let minor = parts[1]
            .parse::<u64>()
            .map_err(|_| crate::error::MantaError::Validation(format!(
                "Invalid minor version in '{}'",
                s
            )))?;
        let patch = parts[2]
            .parse::<u64>()
            .map_err(|_| crate::error::MantaError::Validation(format!(
                "Invalid patch version in '{}'",
                s
            )))?;

        Ok(Self { major, minor, patch })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A version constraint like `^1.0.0`, `>=1.2.0`, `~1.2.3`, or `1.0.0`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReq {
    pub op: Op,
    pub version: Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Exact,      // =1.0.0 or 1.0.0
    Caret,      // ^1.0.0
    Tilde,      // ~1.2.0
    Gte,        // >=1.0.0
    Lte,        // <=1.0.0
    Gt,         // >1.0.0
    Lt,         // <1.0.0
    Wildcard,   // * or 1.x
}

impl VersionReq {
    /// Parse a constraint string
    pub fn parse(s: &str) -> crate::Result<Self> {
        s.parse()
    }

    /// Check if a version matches this constraint
    pub fn matches(&self, version: &Version) -> bool {
        match self.op {
            Op::Exact => version == &self.version,
            Op::Caret => {
                // ^1.2.3 := >=1.2.3, <2.0.0
                // ^0.2.3 := >=0.2.3, <0.3.0
                // ^0.0.3 := >=0.0.3, <0.0.4
                if self.version.major == 0 {
                    if self.version.minor == 0 {
                        version.major == 0
                            && version.minor == 0
                            && version.patch >= self.version.patch
                    } else {
                        version.major == 0
                            && version.minor == self.version.minor
                            && version.patch >= self.version.patch
                    }
                } else {
                    version.major == self.version.major
                        && (*version >= self.version)
                }
            }
            Op::Tilde => {
                // ~1.2.3 := >=1.2.3, <1.3.0
                version.major == self.version.major
                    && version.minor == self.version.minor
                    && version.patch >= self.version.patch
            }
            Op::Gte => version >= &self.version,
            Op::Lte => version <= &self.version,
            Op::Gt => version > &self.version,
            Op::Lt => version < &self.version,
            Op::Wildcard => true,
        }
    }
}

impl FromStr for VersionReq {
    type Err = crate::error::MantaError;

    fn from_str(s: &str) -> crate::Result<Self> {
        let s = s.trim();

        if s == "*" || s == "x" || s == "X" {
            return Ok(VersionReq {
                op: Op::Wildcard,
                version: Version::new(0, 0, 0),
            });
        }

        // Check for operators
        let (op_str, version_str) = if s.starts_with("^=") {
            ("^", &s[2..])
        } else if s.starts_with("~=") {
            ("~", &s[2..])
        } else if s.starts_with(">=") {
            (">=", &s[2..])
        } else if s.starts_with("<=") {
            ("<=", &s[2..])
        } else if s.starts_with('^') {
            ("^", &s[1..])
        } else if s.starts_with('~') {
            ("~", &s[1..])
        } else if s.starts_with('>') {
            (">", &s[1..])
        } else if s.starts_with('<') {
            ("<", &s[1..])
        } else if s.starts_with('=') {
            ("=", &s[1..])
        } else {
            ("=", s)
        };

        let op = match op_str {
            "^" => Op::Caret,
            "~" => Op::Tilde,
            ">=" => Op::Gte,
            "<=" => Op::Lte,
            ">" => Op::Gt,
            "<" => Op::Lt,
            "=" => Op::Exact,
            _ => Op::Exact,
        };

        let version = Version::parse(version_str)?;

        Ok(VersionReq { op, version })
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op_str = match self.op {
            Op::Exact => "=",
            Op::Caret => "^",
            Op::Tilde => "~",
            Op::Gte => ">=",
            Op::Lte => "<=",
            Op::Gt => ">",
            Op::Lt => "<",
            Op::Wildcard => "*",
        };
        if self.op == Op::Wildcard {
            write!(f, "*")
        } else {
            write!(f, "{}{}", op_str, self.version)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parse() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn test_version_parse_with_v() {
        let v = Version::parse("v1.2.3").unwrap();
        assert_eq!(v, Version::new(1, 2, 3));
    }

    #[test]
    fn test_version_parse_invalid() {
        assert!(Version::parse("1.2").is_err());
        assert!(Version::parse("1.2.3.4").is_err());
        assert!(Version::parse("abc").is_err());
    }

    #[test]
    fn test_version_display() {
        assert_eq!(Version::new(1, 2, 3).to_string(), "1.2.3");
    }

    #[test]
    fn test_version_comparison() {
        assert!(Version::new(1, 2, 3) < Version::new(1, 2, 4));
        assert!(Version::new(1, 2, 3) < Version::new(1, 3, 0));
        assert!(Version::new(1, 2, 3) < Version::new(2, 0, 0));
        assert!(Version::new(1, 2, 3) == Version::new(1, 2, 3));
    }

    #[test]
    fn version_req_exact() {
        let req = VersionReq::parse("1.2.3").unwrap();
        assert!(req.matches(&Version::new(1, 2, 3)));
        assert!(!req.matches(&Version::new(1, 2, 4)));
    }

    #[test]
    fn version_req_caret_major_nonzero() {
        let req = VersionReq::parse("^1.2.3").unwrap();
        assert!(req.matches(&Version::new(1, 2, 3)));
        assert!(req.matches(&Version::new(1, 3, 0)));
        assert!(req.matches(&Version::new(1, 9, 9)));
        assert!(!req.matches(&Version::new(2, 0, 0)));
        assert!(!req.matches(&Version::new(1, 2, 2)));
    }

    #[test]
    fn version_req_caret_major_zero() {
        let req = VersionReq::parse("^0.2.3").unwrap();
        assert!(req.matches(&Version::new(0, 2, 3)));
        assert!(req.matches(&Version::new(0, 2, 4)));
        assert!(!req.matches(&Version::new(0, 3, 0)));
        assert!(!req.matches(&Version::new(1, 0, 0)));
    }

    #[test]
    fn version_req_caret_major_minor_zero() {
        let req = VersionReq::parse("^0.0.3").unwrap();
        assert!(req.matches(&Version::new(0, 0, 3)));
        assert!(req.matches(&Version::new(0, 0, 4)));
        assert!(!req.matches(&Version::new(0, 0, 2)));
        assert!(!req.matches(&Version::new(0, 1, 0)));
    }

    #[test]
    fn version_req_tilde() {
        let req = VersionReq::parse("~1.2.3").unwrap();
        assert!(req.matches(&Version::new(1, 2, 3)));
        assert!(req.matches(&Version::new(1, 2, 4)));
        assert!(!req.matches(&Version::new(1, 3, 0)));
        assert!(!req.matches(&Version::new(1, 2, 2)));
    }

    #[test]
    fn version_req_gte() {
        let req = VersionReq::parse(">=1.2.3").unwrap();
        assert!(req.matches(&Version::new(1, 2, 3)));
        assert!(req.matches(&Version::new(2, 0, 0)));
        assert!(!req.matches(&Version::new(1, 2, 2)));
    }

    #[test]
    fn version_req_gt() {
        let req = VersionReq::parse(">1.2.3").unwrap();
        assert!(!req.matches(&Version::new(1, 2, 3)));
        assert!(req.matches(&Version::new(1, 2, 4)));
    }

    #[test]
    fn version_req_lte() {
        let req = VersionReq::parse("<=1.2.3").unwrap();
        assert!(req.matches(&Version::new(1, 2, 3)));
        assert!(req.matches(&Version::new(1, 2, 2)));
        assert!(!req.matches(&Version::new(1, 2, 4)));
    }

    #[test]
    fn version_req_lt() {
        let req = VersionReq::parse("<1.2.3").unwrap();
        assert!(!req.matches(&Version::new(1, 2, 3)));
        assert!(req.matches(&Version::new(1, 2, 2)));
    }

    #[test]
    fn version_req_wildcard() {
        let req = VersionReq::parse("*").unwrap();
        assert!(req.matches(&Version::new(0, 0, 0)));
        assert!(req.matches(&Version::new(99, 99, 99)));
    }

    #[test]
    fn version_req_display() {
        assert_eq!(VersionReq::parse("^1.0.0").unwrap().to_string(), "^1.0.0");
        assert_eq!(VersionReq::parse("~1.0.0").unwrap().to_string(), "~1.0.0");
        assert_eq!(VersionReq::parse(">=1.0.0").unwrap().to_string(), ">=1.0.0");
        assert_eq!(VersionReq::parse("1.0.0").unwrap().to_string(), "=1.0.0");
        assert_eq!(VersionReq::parse("*").unwrap().to_string(), "*");
    }
}
