use std::io;
use std::path::{Path, PathBuf};

pub const ENTRY_NAME: &str = "k100d";

pub trait Autostart: Send {
    fn enable(&self) -> io::Result<()>;
    fn disable(&self) -> io::Result<()>;
    fn is_enabled(&self) -> io::Result<bool>;
}

pub struct Unavailable(pub String);

impl Autostart for Unavailable {
    fn enable(&self) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, self.0.clone()))
    }

    fn disable(&self) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, self.0.clone()))
    }

    fn is_enabled(&self) -> io::Result<bool> {
        Ok(false)
    }
}

pub fn run_command(k100: &Path) -> String {
    format!("\"{}\" daemon", k100.display())
}

pub fn exec_line(k100: &Path) -> String {
    let mut line = String::from("\"");
    for character in k100.to_string_lossy().chars() {
        match character {
            '\\' => line.push_str(r"\\\\"),
            '"' | '`' | '$' => {
                line.push_str(r"\\");
                line.push(character);
            }
            '%' => line.push_str("%%"),
            other => line.push(other),
        }
    }
    line.push_str("\" daemon");
    line
}

pub fn desktop_entry(k100: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=K100 daemon\nComment=Lighting and key actions for the Corsair K100\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exec_line(k100)
    )
}

pub struct XdgEntry {
    dir: PathBuf,
    k100: PathBuf,
}

impl XdgEntry {
    pub fn new(dir: PathBuf, k100: PathBuf) -> Self {
        Self { dir, k100 }
    }

    fn path(&self) -> PathBuf {
        self.dir.join(format!("{ENTRY_NAME}.desktop"))
    }
}

impl Autostart for XdgEntry {
    fn enable(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.path(), desktop_entry(&self.k100))
    }

    fn disable(&self) -> io::Result<()> {
        match std::fs::remove_file(self.path()) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    fn is_enabled(&self) -> io::Result<bool> {
        Ok(self.path().exists())
    }
}

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
pub struct RunKey {
    command: String,
}

#[cfg(windows)]
impl RunKey {
    pub fn new(k100: &Path) -> Self {
        Self { command: run_command(k100) }
    }

    fn open(access: u32) -> io::Result<Option<winreg::RegKey>> {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, access) {
            Ok(key) => Ok(Some(key)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(windows)]
impl Autostart for RunKey {
    fn enable(&self) -> io::Result<()> {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
        key.set_value(ENTRY_NAME, &self.command)
    }

    fn disable(&self) -> io::Result<()> {
        let Some(key) = Self::open(winreg::enums::KEY_WRITE)? else { return Ok(()) };
        match key.delete_value(ENTRY_NAME) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    fn is_enabled(&self) -> io::Result<bool> {
        let Some(key) = Self::open(winreg::enums::KEY_READ)? else { return Ok(false) };
        match key.get_value::<String, _>(ENTRY_NAME) {
            Ok(value) => Ok(value == self.command),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

#[cfg(windows)]
pub fn platform(k100: PathBuf) -> io::Result<Box<dyn Autostart>> {
    Ok(Box::new(RunKey::new(&k100)))
}

#[cfg(unix)]
pub fn platform(k100: PathBuf) -> io::Result<Box<dyn Autostart>> {
    let dirs = directories::BaseDirs::new().ok_or_else(|| io::Error::other("no home directory"))?;
    Ok(Box::new(XdgEntry::new(dirs.config_dir().join("autostart"), k100)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> (tempfile::TempDir, XdgEntry) {
        let dir = tempfile::tempdir().unwrap();
        let entry = XdgEntry::new(dir.path().join("autostart"), PathBuf::from("/opt/my apps/k100"));
        (dir, entry)
    }

    #[test]
    fn the_windows_command_quotes_the_path() {
        let command = run_command(Path::new(r"C:\Program Files\k100\k100.exe"));
        assert_eq!(command, r#""C:\Program Files\k100\k100.exe" daemon"#);
    }

    #[test]
    fn exec_lines_quote_spaces_and_escape_specials() {
        assert_eq!(exec_line(Path::new("/opt/my apps/k100")), r#""/opt/my apps/k100" daemon"#);
        assert_eq!(exec_line(Path::new("/opt/$x/100%/k100")), r#""/opt/\\$x/100%%/k100" daemon"#);
    }

    #[test]
    fn enabling_writes_the_entry_and_is_idempotent() {
        let (_dir, entry) = entry();
        assert!(!entry.is_enabled().unwrap());
        entry.enable().unwrap();
        entry.enable().unwrap();
        assert!(entry.is_enabled().unwrap());
        let text = std::fs::read_to_string(entry.path()).unwrap();
        assert!(text.starts_with("[Desktop Entry]\n"), "{text}");
        assert!(text.contains("Exec=\"/opt/my apps/k100\" daemon\n"), "{text}");
    }

    #[test]
    fn disabling_removes_the_entry_and_tolerates_a_missing_one() {
        let (_dir, entry) = entry();
        entry.disable().unwrap();
        entry.enable().unwrap();
        entry.disable().unwrap();
        assert!(!entry.is_enabled().unwrap());
    }

    #[test]
    fn an_unavailable_entry_says_why() {
        let unavailable = Unavailable("k100 not found".into());
        assert!(!unavailable.is_enabled().unwrap());
        assert!(unavailable.enable().unwrap_err().to_string().contains("k100 not found"));
        assert!(unavailable.disable().is_err());
    }
}
