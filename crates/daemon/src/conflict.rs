use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

const CONFLICTING: &[&str] = &[
    "icue.exe",
    "corsair.service.exe",
    "corsairdevicelisterservice.exe",
    "ckb-next-daemon",
    "openrgb",
    "openrgb.exe",
    "openlinkhub",
    "openlinkhub.exe",
    "signalrgb.exe",
];

pub fn find_conflicts<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut found: Vec<String> = names
        .into_iter()
        .filter(|name| CONFLICTING.contains(&name.to_lowercase().as_str()))
        .map(str::to_owned)
        .collect();
    found.sort_by_key(|name| name.to_lowercase());
    found.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    found
}

pub fn running_conflicts() -> Vec<String> {
    let mut system = System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()));
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let names: Vec<String> = system.processes().values().map(|p| p.name().to_string_lossy().into_owned()).collect();
    find_conflicts(names.iter().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_known_conflicts_case_insensitively() {
        let names = ["explorer.exe", "iCUE.exe", "ICUE.EXE", "OpenRGB.exe", "ckb-next-daemon", "bash"];
        assert_eq!(find_conflicts(names), ["ckb-next-daemon", "iCUE.exe", "OpenRGB.exe"]);
        assert!(find_conflicts(["k100d.exe"]).is_empty());
    }
}
