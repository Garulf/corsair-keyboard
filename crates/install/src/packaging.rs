#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::desktop_entry;

    const RULES: &str = include_str!("../../../packaging/linux/70-k100.rules");
    const SERVICE: &str = include_str!("../../../packaging/linux/k100d.service");
    const AUTOSTART: &str = include_str!("../../../packaging/linux/k100d.desktop");
    const LAUNCHER: &str = include_str!("../../../packaging/linux/k100.desktop");

    fn unix(text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    fn keys(text: &str) -> Vec<String> {
        unix(text).lines().filter_map(|line| line.split_once('=')).map(|(key, _)| key.to_owned()).collect()
    }

    #[test]
    fn the_udev_rules_grant_the_logged_in_user_hidraw_and_uinput_access() {
        let rules = unix(RULES);
        let hidraw = rules.lines().any(|line| {
            line.contains("SUBSYSTEM==\"hidraw\"")
                && line.contains("ATTRS{idVendor}==\"1b1c\"")
                && line.contains("TAG+=\"uaccess\"")
        });
        let uinput = rules.lines().any(|line| line.contains("KERNEL==\"uinput\"") && line.contains("TAG+=\"uaccess\""));
        assert!(hidraw && uinput, "{rules}");
        assert!(rules.lines().all(|line| line.trim().is_empty() || line.contains("==")), "no stray lines");
    }

    #[test]
    fn the_service_runs_k100d_in_the_foreground_and_restarts_it() {
        let service = unix(SERVICE);
        assert!(service.contains("ExecStart=/usr/local/bin/k100d\n"));
        assert!(service.contains("Type=simple\n") && service.contains("Restart=on-failure\n"));
        assert!(service.contains("WantedBy=graphical-session.target\n"));
    }

    #[test]
    fn the_service_does_not_restart_k100d_when_another_instance_holds_the_lock() {
        let service = unix(SERVICE);
        let expected = format!("RestartPreventExitStatus={}\n", ipc::ALREADY_RUNNING);
        assert!(service.contains(&expected), "{service}");
    }

    #[test]
    fn desktop_files_are_complete() {
        let autostart = unix(AUTOSTART);
        let launcher = unix(LAUNCHER);
        for (name, text) in [("k100d.desktop", &autostart), ("k100.desktop", &launcher)] {
            assert!(text.starts_with("[Desktop Entry]\n"), "{name}");
            for needle in ["Type=Application\n", "Name=", "Exec=k100 ", "Terminal=false\n"] {
                assert!(text.contains(needle), "{name} lacks {needle}");
            }
        }
        assert!(autostart.contains("Exec=k100 daemon\n") && launcher.contains("Exec=k100 gui\n"));
    }

    #[test]
    fn the_shipped_autostart_entry_has_the_keys_the_generated_one_has() {
        assert_eq!(keys(&desktop_entry(Path::new("k100"))), keys(AUTOSTART));
    }

    #[test]
    fn packaging_text_has_no_em_dashes_or_trailing_spaces() {
        for text in [RULES, SERVICE, AUTOSTART, LAUNCHER] {
            let text = unix(text);
            assert!(!text.contains('\u{2014}'));
            assert!(text.lines().all(|line| line == line.trim_end()));
            assert!(text.ends_with('\n'));
        }
    }
}
