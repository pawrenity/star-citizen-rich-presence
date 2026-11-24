use std::path::{Path, PathBuf};
use sysinfo::{Disks, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProcess {
    pub pid: u32,
    pub started: u64,
    pub executable: Option<PathBuf>,
}

pub fn running_game(system: &mut System, channel: &str) -> Option<GameProcess> {
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let candidates: Vec<(u32, u64, Option<PathBuf>)> = system
        .processes()
        .iter()
        .filter(|(_, process)| {
            process
                .name()
                .to_string_lossy()
                .eq_ignore_ascii_case("StarCitizen.exe")
        })
        .map(|(pid, process)| {
            (
                pid.as_u32(),
                process.start_time(),
                process.exe().map(Path::to_path_buf),
            )
        })
        .collect();
    pick_game(&candidates, channel)
}

/// Prefers the newest process whose executable is on `channel`. Only when none is known to
/// be on that channel does it fall back to the newest process with an unknown executable path.
/// Processes whose executable is on another channel are never chosen.
fn pick_game(candidates: &[(u32, u64, Option<PathBuf>)], channel: &str) -> Option<GameProcess> {
    candidates
        .iter()
        .filter_map(|(pid, started, exe)| {
            let group = match exe.as_deref().and_then(channel_from_exe) {
                Some(found) if found.eq_ignore_ascii_case(channel) => 0,
                Some(_) => return None,
                None => 1,
            };
            Some((group, *started, *pid, exe))
        })
        .min_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, started, pid, exe)| GameProcess {
            pid,
            started,
            executable: exe.clone(),
        })
}

fn channel_from_exe(exe: &Path) -> Option<&str> {
    exe.parent()?.parent()?.file_name()?.to_str()
}

pub fn log_from_exe(exe: &Path) -> Option<PathBuf> {
    // ...\LIVE\Bin64\StarCitizen.exe
    Some(exe.parent()?.parent()?.join("Game.log"))
}

pub fn candidate_logs(channel: &str, game: Option<&GameProcess>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = game
        .and_then(|g| g.executable.as_deref())
        .and_then(log_from_exe)
    {
        paths.push(path);
    }
    for key in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(root) = std::env::var_os(key) {
            for vendor in ["Star Citizen", "Roberts Space Industries"] {
                paths.push(
                    PathBuf::from(&root)
                        .join(vendor)
                        .join("StarCitizen")
                        .join(channel)
                        .join("Game.log"),
                );
            }
        }
    }
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(key) {
            paths.push(
                PathBuf::from(root)
                    .join("Roberts Space Industries/StarCitizen")
                    .join(channel)
                    .join("Game.log"),
            );
        }
    }
    // Small fixed set on every mounted volume; never recursively scan the user's disk.
    for disk in Disks::new_with_refreshed_list().list() {
        for relative in [
            "Roberts Space Industries/StarCitizen",
            "Program Files/Roberts Space Industries/StarCitizen",
            "Games/StarCitizen",
            "Games/Roberts Space Industries/StarCitizen",
            "StarCitizen",
        ] {
            paths.push(
                disk.mount_point()
                    .join(relative)
                    .join(channel)
                    .join("Game.log"),
            );
        }
    }
    let mut unique = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique
}

pub fn discover_log(channel: &str, game: Option<&GameProcess>) -> Option<PathBuf> {
    let paths = candidate_logs(channel, game);
    // Prefer the active executable's installation even if its log is not created yet.
    if let Some(path) = game
        .and_then(|g| g.executable.as_deref())
        .and_then(log_from_exe)
    {
        return Some(path);
    }
    paths
        .into_iter()
        .filter(|p| p.is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn derives_the_channel_log_from_game_not_launcher() {
        let exe = Path::new("Games/StarCitizen/EPTU/Bin64/StarCitizen.exe");
        assert_eq!(
            log_from_exe(exe).unwrap(),
            PathBuf::from("Games/StarCitizen/EPTU/Game.log")
        );
        assert_eq!(channel_from_exe(exe), Some("EPTU"));
    }
    #[test]
    fn picks_the_newest_process_on_the_requested_channel() {
        let live = |s: &str| {
            Some(PathBuf::from(format!(
                "Games/StarCitizen/{s}/Bin64/StarCitizen.exe"
            )))
        };
        let candidates = [
            (1, 100, live("LIVE")),
            (2, 300, live("PTU")),
            (3, 200, live("LIVE")),
        ];
        let picked = pick_game(&candidates, "LIVE").unwrap();
        assert_eq!(picked.pid, 3);
        assert_eq!(picked.started, 200);
        assert_eq!(picked.executable, live("LIVE"));
    }
    #[test]
    fn unknown_exe_is_only_a_fallback_and_other_channels_are_excluded() {
        let ptu = Some(PathBuf::from("Games/StarCitizen/PTU/Bin64/StarCitizen.exe"));
        let candidates = [(1, 500, None), (2, 100, ptu.clone())];
        // The newer unknown-path process is used only because no LIVE process is known.
        assert_eq!(pick_game(&candidates, "LIVE").unwrap().pid, 1);
        assert_eq!(pick_game(&candidates, "PTU").unwrap().pid, 2);
        let with_match = [
            (1, 500, None),
            (
                2,
                100,
                Some(PathBuf::from(
                    "Games/StarCitizen/LIVE/Bin64/StarCitizen.exe",
                )),
            ),
        ];
        assert_eq!(pick_game(&with_match, "LIVE").unwrap().pid, 2);
        assert!(pick_game(&[(2, 100, ptu)], "LIVE").is_none());
        assert!(pick_game(&[], "LIVE").is_none());
    }
}
