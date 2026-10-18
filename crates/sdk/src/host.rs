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
        let name = file_name(exe);
        self.enabled && !self.blocked.iter().any(|blocked| blocked.eq_ignore_ascii_case(name))
    }
}

fn file_name(path: &str) -> &str {
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
    fn current_frame(&self) -> Option<k100::Frame>;
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
    fn a_disabled_sdk_allows_no_one() {
        assert!(!Policy { enabled: false, ..Policy::default() }.allows("game.exe"));
    }
}
