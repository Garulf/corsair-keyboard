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

pub fn remove_ignored(found: Vec<String>, ignored: &[String]) -> Vec<String> {
    found.into_iter().filter(|name| !ignored.iter().any(|skip| skip.eq_ignore_ascii_case(name))).collect()
}

pub fn find_conflicts_except<'a>(processes: impl IntoIterator<Item = (u32, &'a str)>, own_pid: u32) -> Vec<String> {
    find_conflicts(processes.into_iter().filter(|&(pid, _)| own_pid == 0 || pid != own_pid).map(|(_, name)| name))
}

pub fn running_conflicts(standin_pid: u32) -> Vec<String> {
    let mut system = System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()));
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let processes: Vec<(u32, String)> = system
        .processes()
        .iter()
        .map(|(pid, process)| (pid.as_u32(), process.name().to_string_lossy().into_owned()))
        .collect();
    find_conflicts_except(processes.iter().map(|(pid, name)| (*pid, name.as_str())), standin_pid)
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

    #[test]
    fn ignored_names_are_not_conflicts() {
        let names = ["OpenRGB.exe", "iCUE.exe"];
        assert_eq!(remove_ignored(find_conflicts(names), &["openrgb.exe".to_string()]), ["iCUE.exe"]);
    }

    #[test]
    fn our_standin_is_not_a_conflict_but_another_icue_is() {
        assert!(find_conflicts_except([(7, "iCUE.exe"), (8, "bash")], 7).is_empty());
        assert_eq!(find_conflicts_except([(7, "iCUE.exe"), (9, "iCUE.exe")], 7), ["iCUE.exe"]);
        assert_eq!(find_conflicts_except([(7, "iCUE.exe")], 0), ["iCUE.exe"]);
    }
}
