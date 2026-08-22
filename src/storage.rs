use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn write_private(path: &Path, bytes: &[u8], subject: &str) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("invalid {subject} path"))?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("could not create {subject} directory: {error}"))?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not secure {subject} directory: {error}"))?;

    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("invalid {subject} path"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = directory.join(format!(".{filename}.{}.{}.tmp", std::process::id(), nonce));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("could not create {subject}: {error}"))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("could not write {subject}: {error}"))?;
        fs::rename(&temporary, path).map_err(|error| format!("could not save {subject}: {error}"))
    })();

    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::write_private;

    #[test]
    fn writes_private_files_atomically() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("ask-storage-{}-{nonce}", std::process::id()));
        let path = root.join("ask/value.json");

        write_private(&path, b"first", "test data").unwrap();
        write_private(&path, b"second", "test data").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        fs::remove_dir_all(root).unwrap();
    }
}
