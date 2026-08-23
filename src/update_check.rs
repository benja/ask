use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::{storage, upgrade};

const CHECK_INTERVAL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Cache {
    checked_at: u64,
    latest: Option<String>,
    last_notified: Option<String>,
}

pub struct Check {
    notice: Option<String>,
    notification: Option<(PathBuf, String)>,
}

impl Check {
    pub fn start(enabled: bool) -> Self {
        if !enabled {
            return Self::disabled();
        }
        let Some(cache_path) = cache_path() else {
            return Self::disabled();
        };
        let now = now();
        let cache = read_cache(&cache_path).unwrap_or_default();
        if let Some(notice) = notice_for(&cache) {
            return Self {
                notice: Some(notice),
                notification: Some((cache_path, cache.latest.unwrap_or_default())),
            };
        }
        if begin_refresh(&cache_path, &cache, now) {
            std::thread::spawn(move || {
                if let Ok(latest) = upgrade::latest_release_version() {
                    let _ = store_latest(&cache_path, now, latest);
                }
            });
        }
        Self::disabled()
    }

    pub fn notice(self) -> Option<String> {
        if let Some((path, version)) = self.notification {
            mark_notified(&path, &version).ok()?;
        }
        self.notice
    }

    fn disabled() -> Self {
        Self {
            notice: None,
            notification: None,
        }
    }
}

fn begin_refresh(path: &Path, fallback: &Cache, checked_at: u64) -> bool {
    let mut cache = read_cache(path).unwrap_or_else(|| fallback.clone());
    if checked_at.saturating_sub(cache.checked_at) < CHECK_INTERVAL_SECONDS {
        return false;
    }
    cache.checked_at = checked_at;
    write_cache(path, &cache).is_ok()
}

fn store_latest(path: &Path, checked_at: u64, latest: String) -> Result<(), String> {
    let mut cache = read_cache(path).unwrap_or_default();
    cache.checked_at = cache.checked_at.max(checked_at);
    cache.latest = Some(latest);
    write_cache(path, &cache)
}

fn mark_notified(path: &Path, version: &str) -> Result<(), String> {
    let mut cache = read_cache(path).unwrap_or_default();
    cache.last_notified = Some(version.to_owned());
    write_cache(path, &cache)
}

fn notice_for(cache: &Cache) -> Option<String> {
    let latest = cache.latest.as_deref()?;
    if !upgrade::is_newer_release(latest) || cache.last_notified.as_deref() == Some(latest) {
        return None;
    }
    let current = env!("CARGO_PKG_VERSION");
    Some(format!(
        "ask {latest} is available (you have {current})\nrun `ask --upgrade` to install it"
    ))
}

fn cache_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path).join("ask/update.json"));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".cache/ask/update.json"))
}

fn read_cache(path: &Path) -> Option<Cache> {
    let bytes = std::fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    Some(Cache {
        checked_at: value.get("checked_at")?.as_u64()?,
        latest: optional_string(&value, "latest")?,
        last_notified: optional_string(&value, "last_notified")?,
    })
}

fn optional_string(value: &Value, key: &str) -> Option<Option<String>> {
    match value.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(value.clone())),
        Some(_) => None,
    }
}

fn write_cache(path: &Path, cache: &Cache) -> Result<(), String> {
    let value = json!({
        "checked_at": cache.checked_at,
        "latest": cache.latest,
        "last_notified": cache.last_notified,
    });
    let bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| format!("could not encode update cache: {error}"))?;
    storage::write_private(path, &bytes, "update cache")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{
        Cache, Check, begin_refresh, notice_for, optional_string, read_cache, store_latest,
        write_cache,
    };
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn update_cache_round_trips_in_an_isolated_directory() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ask-update-test-{}-{unique}", std::process::id()));
        let path = directory.join("nested/update.json");
        let cache = Cache {
            checked_at: 42,
            latest: Some("0.2.0".into()),
            last_notified: None,
        };

        write_cache(&path, &cache).unwrap();
        assert_eq!(read_cache(&path), Some(cache));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_optional_cache_fields_are_rejected() {
        assert_eq!(optional_string(&json!({}), "latest"), Some(None));
        assert_eq!(optional_string(&json!({"latest": 1}), "latest"), None);
    }

    #[test]
    fn each_new_version_is_announced_only_once() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ask-notice-test-{}-{unique}", std::process::id()));
        let path = directory.join("update.json");
        let cache = Cache {
            checked_at: 42,
            latest: Some("999.0.0".into()),
            last_notified: None,
        };
        write_cache(&path, &cache).unwrap();
        let first = Check {
            notice: notice_for(&cache),
            notification: Some((path.clone(), "999.0.0".into())),
        };

        let expected = format!(
            "ask 999.0.0 is available (you have {})\nrun `ask --upgrade` to install it",
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(first.notice().as_deref(), Some(expected.as_str()));
        let notified = read_cache(&path).unwrap();
        assert_eq!(notified.last_notified.as_deref(), Some("999.0.0"));
        let second = Check {
            notice: notice_for(&notified),
            notification: None,
        };
        assert!(second.notice().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn refresh_is_throttled_before_background_work_starts() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ask-throttle-test-{}-{unique}", std::process::id()));
        let path = directory.join("update.json");
        let cache = Cache::default();

        assert!(begin_refresh(&path, &cache, 100_000));
        assert!(!begin_refresh(&path, &cache, 100_001));
        assert_eq!(read_cache(&path).unwrap().checked_at, 100_000);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn refresh_preserves_notification_state_written_by_another_run() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ask-merge-test-{}-{unique}", std::process::id()));
        let path = directory.join("update.json");
        let cache = Cache {
            checked_at: 100,
            latest: Some("0.2.0".into()),
            last_notified: Some("0.2.0".into()),
        };
        write_cache(&path, &cache).unwrap();

        store_latest(&path, 200, "0.3.0".into()).unwrap();

        let refreshed = read_cache(&path).unwrap();
        assert_eq!(refreshed.latest.as_deref(), Some("0.3.0"));
        assert_eq!(refreshed.last_notified.as_deref(), Some("0.2.0"));
        fs::remove_dir_all(directory).unwrap();
    }
}
