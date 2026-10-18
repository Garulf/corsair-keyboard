use crate::device::DeviceSnapshot;
use crate::ids::{AccessLevel, MacroKey};
use crate::server::ClientId;
use engine::ClientLighting;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub enabled: bool,
    pub exclusive_control: bool,
    pub blocked: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self { enabled: true, exclusive_control: true, blocked: Vec::new() }
    }
}

impl Policy {
    pub fn allows(&self, exe: &str) -> bool {
        let name = exe_file_name(exe);
        self.enabled && !self.blocked.iter().any(|blocked| exe_file_name(blocked).eq_ignore_ascii_case(name))
    }
}

pub fn exe_file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSummary {
    pub id: ClientId,
    pub exe: String,
    pub refused: bool,
    pub access: AccessLevel,
    pub priority: u8,
    pub intercepted: Vec<MacroKey>,
    pub subscribed: bool,
}

pub trait SdkHost {
    fn device(&self) -> Option<DeviceSnapshot>;
    fn policy(&self) -> Policy;
    fn set_lighting(&mut self, lighting: ClientLighting);
    fn set_intercepted(&mut self, leds: Vec<u8>);
    fn clients_changed(&mut self, clients: Vec<ClientSummary>);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_policy_allows_everyone() {
        assert_eq!(Policy::default(), Policy { enabled: true, exclusive_control: true, blocked: vec![] });
        assert!(Policy::default().allows("game.exe"));
    }

    #[test]
    fn blocking_matches_the_file_name_ignoring_case() {
        let policy = Policy { blocked: vec!["StreamDeck.exe".into()], ..Policy::default() };
        assert!(!policy.allows("streamdeck.EXE"));
        assert!(!policy.allows(r"C:\Program Files\Elgato\StreamDeck\StreamDeck.exe"));
        assert!(!policy.allows("/opt/StreamDeck.exe"));
        assert!(policy.allows("StreamDeck2.exe"));
    }

    #[test]
    fn a_blocked_entry_stored_as_a_path_matches_by_its_file_name() {
        let policy =
            Policy { blocked: vec![r"C:\Program Files\Elgato\StreamDeck\StreamDeck.exe".into()], ..Policy::default() };
        assert!(!policy.allows("streamdeck.exe"));
        assert!(!policy.allows(r"D:\Other\StreamDeck.exe"));
        assert!(policy.allows("game.exe"));
    }

    #[test]
    fn a_disabled_sdk_allows_no_one() {
        assert!(!Policy { enabled: false, ..Policy::default() }.allows("game.exe"));
    }
}
