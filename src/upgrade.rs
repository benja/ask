use std::cmp::Ordering;
use std::ffi::OsStr;
use std::fmt;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const RELEASE_URL: &str = "https://api.github.com/repos/benja/ask/releases/latest";
const USER_AGENT: &str = concat!("ask/", env!("CARGO_PKG_VERSION"));
const INSTALLER: &[u8] = include_bytes!("../install.sh");

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Version([u64; 3]);

impl Version {
    fn parse(value: &str) -> Result<Self, String> {
        let mut parts = value.split('.');
        let version = Self([
            component(parts.next(), value)?,
            component(parts.next(), value)?,
            component(parts.next(), value)?,
        ]);
        if parts.next().is_some() {
            return Err(format!("invalid release version '{value}'"));
        }
        Ok(version)
    }

    fn from_tag(tag: &str) -> Result<Self, String> {
        let version = tag
            .strip_prefix('v')
            .ok_or_else(|| format!("latest GitHub release has invalid tag '{tag}'"))?;
        Self::parse(version).map_err(|_| format!("latest GitHub release has invalid tag '{tag}'"))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}

pub fn run() -> Result<String, String> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| "ask was built with an unsupported version".to_string())?;
    let latest = latest_release()?;
    apply_upgrade(current, latest, install)
}

fn apply_upgrade(
    current: Version,
    latest: Version,
    install: impl FnOnce(Version) -> Result<(), String>,
) -> Result<String, String> {
    match latest.cmp(&current) {
        Ordering::Equal => Ok(format!("ask is already up to date (v{current})")),
        Ordering::Less => Ok(format!(
            "ask v{current} is newer than the latest release (v{latest})"
        )),
        Ordering::Greater => {
            install(latest)?;
            Ok(format!("updated ask to v{latest}"))
        }
    }
}

fn latest_release() -> Result<Version, String> {
    let output = fetch_release()?;
    parse_release(&output)
}

pub(crate) fn latest_release_version() -> Result<String, String> {
    latest_release().map(|version| version.to_string())
}

pub(crate) fn is_newer_release(latest: &str) -> bool {
    let Ok(current) = Version::parse(env!("CARGO_PKG_VERSION")) else {
        return false;
    };
    Version::parse(latest).is_ok_and(|latest| latest > current)
}

fn fetch_release() -> Result<Vec<u8>, String> {
    match Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-A",
            USER_AGENT,
            RELEASE_URL,
        ])
        .output()
    {
        Ok(output) => checked_output("curl", output),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Command::new("wget")
            .args(["-T", "30", "-t", "1", "-qO-", RELEASE_URL])
            .output()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    "curl or wget is required to check for upgrades".to_string()
                } else {
                    format!("could not start wget: {error}")
                }
            })
            .and_then(|output| checked_output("wget", output)),
        Err(error) => Err(format!("could not start curl: {error}")),
    }
}

fn checked_output(program: &str, output: Output) -> Result<Vec<u8>, String> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    Err(if detail.is_empty() {
        format!(
            "could not check GitHub releases: {program} exited with {}",
            output.status
        )
    } else {
        format!("could not check GitHub releases: {detail}")
    })
}

fn parse_release(output: &[u8]) -> Result<Version, String> {
    let release: Value = serde_json::from_slice(output)
        .map_err(|error| format!("could not parse GitHub release: {error}"))?;
    let tag = release
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "GitHub latest release is missing 'tag_name'".to_string())?;
    Version::from_tag(tag)
}

fn install(version: Version) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the current ask executable: {error}"))?;
    let directory = install_directory(&executable)?;
    let version = version.to_string();
    let mut child = Command::new("/bin/sh")
        .args(["-s", "--", version.as_str()])
        .env("ASK_INSTALL_DIR", directory)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start the installer: {error}"))?;

    let write = child
        .stdin
        .take()
        .expect("piped installer input is available")
        .write_all(INSTALLER);
    if let Err(error) = write {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("could not run the installer: {error}"));
    }

    let status = child
        .wait()
        .map_err(|error| format!("could not wait for the installer: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("upgrade failed".into())
    }
}

fn install_directory(executable: &Path) -> Result<&Path, String> {
    if executable.file_name() != Some(OsStr::new("ask")) {
        return Err(format!(
            "cannot upgrade renamed executable '{}'; reinstall ask manually",
            executable.display()
        ));
    }
    executable
        .parent()
        .ok_or_else(|| "could not determine ask's install directory".to_string())
}

fn component(part: Option<&str>, value: &str) -> Result<u64, String> {
    let part = part.ok_or_else(|| format!("invalid release version '{value}'"))?;
    if !part.bytes().all(|byte| byte.is_ascii_digit()) || (part.len() > 1 && part.starts_with('0'))
    {
        return Err(format!("invalid release version '{value}'"));
    }
    part.parse()
        .map_err(|_| format!("invalid release version '{value}'"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Version, apply_upgrade, install_directory, parse_release};

    fn version(value: &str) -> Version {
        Version::parse(value).unwrap()
    }

    #[test]
    fn parses_strict_release_tags() {
        assert_eq!(
            parse_release(br#"{"tag_name":"v0.10.2","other":true}"#).unwrap(),
            version("0.10.2")
        );
        for tag in [
            "0.1.0",
            "v0.1",
            "v0.1.0.0",
            "v0.01.0",
            "v+1.0.0",
            "v0.1.0-beta.1",
            "v0.1.0/../../bin",
            "v18446744073709551616.0.0",
        ] {
            let response = format!(r#"{{"tag_name":"{tag}"}}"#);
            assert!(
                parse_release(response.as_bytes()).is_err(),
                "accepted {tag}"
            );
        }
    }

    #[test]
    fn rejects_invalid_release_responses() {
        assert!(parse_release(b"not json").is_err());
        assert!(parse_release(br#"{}"#).is_err());
        assert!(parse_release(br#"{"tag_name":1}"#).is_err());
    }

    #[test]
    fn upgrades_only_to_a_newer_release() {
        let mut installed = None;
        let message = apply_upgrade(version("0.9.9"), version("0.10.0"), |latest| {
            installed = Some(latest);
            Ok(())
        })
        .unwrap();

        assert_eq!(installed, Some(version("0.10.0")));
        assert_eq!(message, "updated ask to v0.10.0");
    }

    #[test]
    fn current_and_newer_local_versions_never_install() {
        let current = apply_upgrade(version("0.1.0"), version("0.1.0"), |_| {
            panic!("must not install an equal version")
        })
        .unwrap();
        let newer = apply_upgrade(version("1.0.0"), version("0.9.9"), |_| {
            panic!("must not downgrade")
        })
        .unwrap();

        assert_eq!(current, "ask is already up to date (v0.1.0)");
        assert_eq!(
            newer,
            "ask v1.0.0 is newer than the latest release (v0.9.9)"
        );
    }

    #[test]
    fn installer_errors_are_preserved() {
        let error = apply_upgrade(version("0.1.0"), version("0.2.0"), |_| {
            Err("install failed".into())
        })
        .unwrap_err();

        assert_eq!(error, "install failed");
    }

    #[test]
    fn upgrades_only_an_executable_named_ask() {
        assert_eq!(
            install_directory(Path::new("/tmp/ask")).unwrap(),
            Path::new("/tmp")
        );
        assert!(install_directory(Path::new("/tmp/renamed")).is_err());
    }
}
