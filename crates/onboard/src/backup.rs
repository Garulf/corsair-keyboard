use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{FREE_USE_IDS, OnboardError};

const PROFILES_LIST_ID: u16 = 0x0F;
const ACTIVE_FILE: &str = "active.txt";
const SECONDS_PER_DAY: u64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub profiles_list: Vec<u8>,
    pub files: BTreeMap<u16, Vec<u8>>,
    pub active: Option<u32>,
}

impl Snapshot {
    pub fn write_to(&self, dir: &Path) -> Result<(), OnboardError> {
        if dir.exists() && std::fs::read_dir(dir)?.next().is_some() {
            let message = format!("{} already holds files", dir.display());
            return Err(std::io::Error::new(ErrorKind::AlreadyExists, message).into());
        }
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join(file_name(PROFILES_LIST_ID)), &self.profiles_list)?;
        for (id, bytes) in &self.files {
            std::fs::write(dir.join(file_name(*id)), bytes)?;
        }
        if let Some(active) = self.active {
            std::fs::write(dir.join(ACTIVE_FILE), format!("{active}\n"))?;
        }
        Ok(())
    }

    pub fn read_from(dir: &Path) -> Result<Snapshot, OnboardError> {
        let not_a_backup = || OnboardError::NotABackup(dir.to_path_buf());
        let mut profiles_list = None;
        let mut files = BTreeMap::new();
        let mut active = None;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| not_a_backup())?;
            if !entry.file_type()?.is_file() {
                return Err(not_a_backup());
            }
            if name == ACTIVE_FILE {
                let text = std::fs::read_to_string(entry.path())?;
                active = Some(text.trim().parse().map_err(|_| not_a_backup())?);
                continue;
            }
            let id = file_id(&name).ok_or_else(not_a_backup)?;
            let bytes = std::fs::read(entry.path())?;
            if id == PROFILES_LIST_ID {
                profiles_list = Some(bytes);
            } else if FREE_USE_IDS.contains(&id) {
                files.insert(id, bytes);
            } else {
                return Err(OnboardError::NotFreeUse(id));
            }
        }
        let profiles_list = profiles_list.ok_or_else(not_a_backup)?;
        Ok(Snapshot { profiles_list, files, active })
    }
}

fn file_name(id: u16) -> String {
    format!("{id:04x}.bin")
}

fn file_id(name: &str) -> Option<u16> {
    let stem = name.strip_suffix(".bin")?;
    let is_hex = stem.len() == 4 && stem.bytes().all(|byte| byte.is_ascii_hexdigit());
    is_hex.then(|| u16::from_str_radix(stem, 16).ok()).flatten()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backups {
    root: PathBuf,
}

impl Backups {
    pub fn under(config_root: &Path) -> Backups {
        Backups { root: config_root.join("onboard-backup") }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save(&self, snapshot: &Snapshot, now: SystemTime) -> Result<PathBuf, OnboardError> {
        let stamp = utc_stamp(now);
        let dir = std::iter::once(stamp.clone())
            .chain((2..).map(|n| format!("{stamp}-{n}")))
            .map(|name| self.root.join(name))
            .find(|dir| !dir.exists())
            .expect("an unused folder name exists");
        snapshot.write_to(&dir)?;
        Ok(dir)
    }
}

pub fn utc_stamp(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let (year, month, day) = civil_date(seconds / SECONDS_PER_DAY);
    let of_day = seconds % SECONDS_PER_DAY;
    let (hour, minute, second) = (of_day / 3600, of_day / 60 % 60, of_day % 60);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

fn civil_date(days_since_epoch: u64) -> (u64, u64, u64) {
    const DAYS_PER_ERA: u64 = 146_097;
    let days = days_since_epoch + 719_468;
    let era = days / DAYS_PER_ERA;
    let day_of_era = days % DAYS_PER_ERA;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            profiles_list: vec![0x13, 0, 1, 0, 0x70, 0x6D],
            files: [(0x6D70, vec![1, 2, 3])].into(),
            active: Some(0),
        }
    }

    #[test]
    fn utc_stamps_sort_and_name_the_second() {
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(1_792_238_461)), "20261017T120101Z");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(1_709_251_199)), "20240229T235959Z");
    }

    #[test]
    fn utc_stamps_cover_the_epoch_and_a_century_leap_day() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101T000000Z");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "20000229T000000Z");
    }

    #[test]
    fn snapshots_round_trip_through_a_folder() {
        let temp = tempfile::tempdir().unwrap();
        let snapshot = sample();
        snapshot.write_to(&temp.path().join("b")).unwrap();
        assert_eq!(Snapshot::read_from(&temp.path().join("b")).unwrap().files, snapshot.files);
        assert!(snapshot.write_to(&temp.path().join("b")).is_err());
    }

    #[test]
    fn a_snapshot_round_trips_every_field_and_names_files_in_lowercase_hex() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("nested").join("b");
        let snapshot = sample();
        snapshot.write_to(&dir).unwrap();
        assert_eq!(std::fs::read(dir.join("6d70.bin")).unwrap(), [1, 2, 3]);
        assert_eq!(std::fs::read(dir.join("000f.bin")).unwrap(), snapshot.profiles_list);
        assert_eq!(Snapshot::read_from(&dir).unwrap(), snapshot);
    }

    #[test]
    fn an_existing_empty_folder_is_used_and_a_missing_active_index_reads_as_none() {
        let temp = tempfile::tempdir().unwrap();
        let snapshot = Snapshot { active: None, ..sample() };
        snapshot.write_to(temp.path()).unwrap();
        assert!(!temp.path().join("active.txt").exists());
        assert_eq!(Snapshot::read_from(temp.path()).unwrap(), snapshot);
    }

    #[test]
    fn folders_without_a_list_or_with_unknown_entries_are_not_backups() {
        let temp = tempfile::tempdir().unwrap();
        assert!(matches!(Snapshot::read_from(temp.path()), Err(OnboardError::NotABackup(_))));
        sample().write_to(&temp.path().join("b")).unwrap();
        std::fs::write(temp.path().join("b").join("notes.txt"), "hi").unwrap();
        assert!(matches!(Snapshot::read_from(&temp.path().join("b")), Err(OnboardError::NotABackup(_))));
        sample().write_to(&temp.path().join("c")).unwrap();
        std::fs::write(temp.path().join("c").join("active.txt"), "first").unwrap();
        assert!(matches!(Snapshot::read_from(&temp.path().join("c")), Err(OnboardError::NotABackup(_))));
    }

    #[test]
    fn a_file_outside_the_free_use_range_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        sample().write_to(temp.path()).unwrap();
        std::fs::write(temp.path().join("0025.bin"), [0x1D, 0]).unwrap();
        assert!(matches!(Snapshot::read_from(temp.path()), Err(OnboardError::NotFreeUse(0x25))));
    }

    #[test]
    fn backups_go_under_the_config_root_with_a_numbered_suffix_when_taken() {
        let temp = tempfile::tempdir().unwrap();
        let backups = Backups::under(temp.path());
        assert_eq!(backups.root(), temp.path().join("onboard-backup"));
        let now = UNIX_EPOCH + Duration::from_secs(1_792_238_461);
        let names: Vec<String> = (0..3)
            .map(|_| backups.save(&sample(), now).unwrap().file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["20261017T120101Z", "20261017T120101Z-2", "20261017T120101Z-3"]);
    }
}
