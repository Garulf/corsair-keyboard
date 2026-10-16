use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => {
                Err(io::Error::new(io::ErrorKind::AlreadyExists, "k100d is already running"))
            }
            Err(TryLockError::Error(error)) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_holder_is_refused_until_the_first_lets_go() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("k100d.lock");
        let first = InstanceLock::acquire(&path).unwrap();
        let refused = InstanceLock::acquire(&path).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
        assert!(refused.to_string().contains("already running"), "{refused}");
        drop(first);
        InstanceLock::acquire(&path).unwrap();
    }

    #[test]
    fn locks_on_different_paths_do_not_interfere() {
        let folder = tempfile::tempdir().unwrap();
        let _first = InstanceLock::acquire(&folder.path().join("a.lock")).unwrap();
        InstanceLock::acquire(&folder.path().join("b.lock")).unwrap();
    }

    #[test]
    fn the_lock_file_sits_next_to_the_socket() {
        let path = crate::lock_path();
        assert_eq!(path.file_name().unwrap(), "k100d.lock");
        if let Some(socket) = crate::socket_path() {
            assert_eq!(path.parent(), socket.parent());
        } else {
            assert_eq!(path, std::env::temp_dir().join("k100d.lock"));
        }
    }
}
