use oxyris_ipc::ops::SystemInfoResult;

pub fn info() -> SystemInfoResult {
    SystemInfoResult {
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        os: std::env::consts::OS.to_owned(),
        kernel: uname_release().unwrap_or_default(),
        arch: std::env::consts::ARCH.to_owned(),
        hostname: hostname().unwrap_or_default(),
        cwd: std::env::current_dir()
            .ok()
            .and_then(|p| p.into_os_string().into_string().ok())
            .unwrap_or_default(),
        home: std::env::var("HOME").unwrap_or_default(),
        user: std::env::var("USER").unwrap_or_default(),
    }
}

#[cfg(target_os = "linux")]
fn uname_release() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_owned())
}

#[cfg(not(target_os = "linux"))]
fn uname_release() -> Option<String> {
    None
}

fn hostname() -> Option<String> {
    // Read from /etc/hostname on Linux; fall back to HOSTNAME env var.
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/etc/hostname") {
            return Some(s.trim().to_owned());
        }
    }
    std::env::var("HOSTNAME").ok()
}
