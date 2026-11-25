use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use super::common::print_json_line;

pub(crate) const SOCKET_UNIT: &str = "aurelia-daemon.socket";
pub(crate) const SERVICE_UNIT: &str = "aurelia-daemon.service";

fn unit_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| aurelia::core::config::home_dir().ok().map(|h| h.join(".config")))
        .context("no config directory for the user's systemd units")?;
    Ok(base.join("systemd").join("user"))
}

pub(crate) fn socket_unit(endpoint: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Aurelia session daemon socket\n\
         \n\
         [Socket]\n\
         ListenStream={endpoint}\n\
         SocketMode=0600\n\
         DirectoryMode=0700\n\
         \n\
         [Install]\n\
         WantedBy=sockets.target\n"
    )
}

pub(crate) fn service_unit(exe: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=Aurelia session daemon (Steam session and bridge for games)\n\
         Documentation=https://github.com/rafael/aurelia\n\
         Requires={SOCKET_UNIT}\n\
         After={SOCKET_UNIT}\n\
         StartLimitIntervalSec=0\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} daemon\n\
         Nice=10\n\
         IOSchedulingClass=best-effort\n\
         IOSchedulingPriority=7\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         SuccessExitStatus=SIGKILL SIGTERM\n\
         Environment=AURELIA_WARMUP_DELAY=20\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe = exe.display()
    )
}

fn systemctl(args: &[&str]) -> Result<std::process::Output> {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("could not run `systemctl --user` (is systemd installed? Without it, start the daemon from your session script with `aurelia daemon start`)")
}

fn ok(args: &[&str]) -> Result<()> {
    let out = systemctl(args)?;
    if !out.status.success() {
        bail!("`systemctl --user {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

fn state(unit: &str, what: &str) -> String {
    systemctl(&[what, unit]).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

pub(crate) fn cmd_service_install(json: bool) -> Result<()> {
    let exe = std::env::current_exe().context("could not find this executable")?;
    let endpoint = crate::daemon::endpoint_path();
    let dir = unit_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    aurelia::core::atomic::write_atomic(&dir.join(SOCKET_UNIT), socket_unit(&endpoint).as_bytes())?;
    aurelia::core::atomic::write_atomic(&dir.join(SERVICE_UNIT), service_unit(&exe).as_bytes())?;
    ok(&["daemon-reload"])?;
    ok(&["enable", "--now", SOCKET_UNIT])?;
    ok(&["enable", SERVICE_UNIT])?;
    let stale: Vec<u32> = crate::proc_admin::find_aurelia_processes().into_iter().filter(|p| p.is_daemon).map(|p| p.pid).collect();
    let _ = crate::proc_admin::kill_pids(&stale);
    ok(&["restart", SOCKET_UNIT])?;
    ok(&["start", "--no-block", SERVICE_UNIT])?;
    if json {
        print_json_line(&serde_json::json!({ "installed": true, "socket": endpoint, "units": [SOCKET_UNIT, SERVICE_UNIT], "dir": dir }));
    } else {
        cli_println!("Installed {SOCKET_UNIT} and {SERVICE_UNIT} in {}", dir.display());
        cli_println!("The daemon starts on the first connection and at every login, in the background; nothing waits for it.");
    }
    Ok(())
}

pub(crate) fn cmd_service_uninstall(json: bool) -> Result<()> {
    let dir = unit_dir()?;
    let _ = systemctl(&["disable", "--now", SERVICE_UNIT]);
    let _ = systemctl(&["disable", "--now", SOCKET_UNIT]);
    for unit in [SERVICE_UNIT, SOCKET_UNIT] {
        let _ = std::fs::remove_file(dir.join(unit));
    }
    let _ = systemctl(&["daemon-reload"]);
    if json {
        print_json_line(&serde_json::json!({ "installed": false }));
    } else {
        cli_println!("Removed the aurelia daemon units. The next command starts a daemon on its own, as before.");
    }
    Ok(())
}

pub(crate) fn cmd_service_status(json: bool) -> Result<()> {
    let installed = crate::daemon::systemd_managed();
    let socket_active = state(SOCKET_UNIT, "is-active");
    let service_active = state(SERVICE_UNIT, "is-active");
    let enabled = state(SERVICE_UNIT, "is-enabled");
    if json {
        print_json_line(&serde_json::json!({ "installed": installed, "socket": socket_active, "service": service_active, "enabled": enabled }));
    } else if !installed {
        cli_println!("Not installed (run `aurelia service install`).");
    } else {
        cli_println!("socket:  {socket_active}\nservice: {service_active} ({enabled} at login)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_never_blocks_the_boot_or_waits_for_the_network() {
        let unit = service_unit(Path::new("/usr/bin/aurelia"));
        assert!(unit.contains("Type=simple"), "{unit}");
        for forbidden in ["Type=notify", "Type=oneshot", "network-online", "Before=", "RemainAfterExit", "WantedBy=graphical", "WantedBy=multi-user"] {
            assert!(!unit.contains(forbidden), "a unit that waits or orders itself against the boot: {forbidden}");
        }
        assert!(unit.contains("ExecStart=/usr/bin/aurelia daemon"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("Restart=on-failure") && unit.contains("RestartSec=30"), "a failure does not become a restart storm");
        assert!(unit.contains("StartLimitIntervalSec=0"));
        assert!(unit.contains("SuccessExitStatus=SIGKILL SIGTERM"));
        assert!(!unit.contains("CPUSchedulingPolicy=idle"), "the daemon must answer the shim while the CPU is busy with the game");
    }

    #[test]
    fn the_socket_unit_listens_on_the_path_the_clients_use() {
        let unit = socket_unit("/run/user/1000/aurelia-1000.sock");
        assert!(unit.contains("ListenStream=/run/user/1000/aurelia-1000.sock"));
        assert!(unit.contains("SocketMode=0600"), "only the owner connects");
    }
}
