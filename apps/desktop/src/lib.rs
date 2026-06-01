//! Oxyris desktop backend library.
//!
//! `main.rs` is a thin entry point that calls [`run`]. Keeping the runtime in a
//! library crate keeps the surface easy to integration-test and lets the Tauri
//! mobile pipeline (if we ever want it) re-use the same entry point.

mod app_state;
mod domain;
mod infra;
mod tauri_commands;

use oxyris_procutil::HideConsole;
use tauri::Manager;

use crate::app_state::AppState;

/// Claude Code ships a preview PowerShell tool gated behind
/// `CLAUDE_CODE_USE_POWERSHELL_TOOL=1`. When the host has PowerShell 7+, set it
/// in our own process environment so every `claude` we spawn inherits it — the
/// pure-mode PTY and the structured provider alike (both inherit our env; only
/// WSL sessions don't, since the var doesn't cross the `wsl.exe` boundary, which
/// is correct — the tool is for the Windows host shell). A user who already set
/// the var wins; we never override an explicit choice.
fn enable_powershell_tool_if_available() {
    if std::env::var_os("CLAUDE_CODE_USE_POWERSHELL_TOOL").is_some() {
        return;
    }
    if !host_has_pwsh7() {
        return;
    }
    // SAFETY: called at the very top of `run()`, before the Tauri builder or any
    // worker thread exists. The process is single-threaded here, so mutating the
    // environment cannot race another thread reading it.
    unsafe {
        std::env::set_var("CLAUDE_CODE_USE_POWERSHELL_TOOL", "1");
    }
}

/// True when `pwsh` resolves on PATH and reports major version >= 7. The `pwsh`
/// binary is PowerShell 6+, but the tool requires 7+, so verify the major once.
fn host_has_pwsh7() -> bool {
    let Ok(path) = which::which("pwsh.exe").or_else(|_| which::which("pwsh")) else {
        return false;
    };
    let Ok(out) = std::process::Command::new(&path)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$PSVersionTable.PSVersion.Major",
        ])
        .hide_console()
        .output()
    else {
        return false;
    };
    out.status.success()
        && String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u32>()
            .map(|major| major >= 7)
            .unwrap_or(false)
}

pub fn run() {
    // Enable Claude Code's PowerShell tool for every `claude` we spawn when the
    // host has PowerShell 7+. Done first, while the process is single-threaded
    // (set_var). See `enable_powershell_tool_if_available`.
    enable_powershell_tool_if_available();

    // Logging is installed inside AppState::initialize once we know the
    // data dir. Until then, stdlib println!/eprintln! are the only channel —
    // that's fine, we don't do anything interesting before setup().
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            tauri_commands::greet,
            tauri_commands::project::project_create,
            tauri_commands::project::project_clone,
            tauri_commands::project::project_rename,
            tauri_commands::project::project_delete,
            tauri_commands::project::project_list,
            tauri_commands::project::project_set_logo,
            tauri_commands::project::project_set_workspace,
            tauri_commands::project::project_reorder,
            tauri_commands::project::project_autodetect_logo,
            tauri_commands::project::project_logo_bytes,
            tauri_commands::environment::environment_list,
            tauri_commands::environment::path_to_posix,
            tauri_commands::environment::path_to_windows,
            tauri_commands::environment::wsl_system_info,
            tauri_commands::environment::wsl_fs_stat,
            tauri_commands::environment::wsl_fs_walk,
            tauri_commands::session::session_start,
            tauri_commands::session::session_send_message,
            tauri_commands::session::session_interrupt,
            tauri_commands::session::session_approve_tool,
            tauri_commands::session::session_reject_tool,
            tauri_commands::session::session_stop,
            tauri_commands::session::session_resume,
            tauri_commands::session::session_rename,
            tauri_commands::session::session_delete,
            tauri_commands::session::session_toggle_pin,
            tauri_commands::session::session_set_env_mode,
            tauri_commands::session::session_list,
            tauri_commands::session::session_get,
            tauri_commands::session::session_turn_diff,
            tauri_commands::session::session_turn_revert,
            tauri_commands::worktree::worktree_create,
            tauri_commands::worktree::worktree_remove,
            tauri_commands::worktree::worktree_list,
            tauri_commands::worktree::git_list_branches,
            tauri_commands::worktree::git_list_worktrees,
            tauri_commands::validate::project_validate_path,
            tauri_commands::settings::settings_provider_discover,
            tauri_commands::settings::settings_logs_dir,
            tauri_commands::settings::settings_keybindings_path,
            tauri_commands::settings::settings_keybindings_read,
            tauri_commands::settings::settings_keybindings_write,
            tauri_commands::terminal::terminal_spawn,
            tauri_commands::terminal::claude_pty_spawn,
            tauri_commands::terminal::terminal_write,
            tauri_commands::terminal::terminal_resize,
            tauri_commands::terminal::terminal_kill,
            tauri_commands::terminal::terminal_list,
            tauri_commands::terminal::terminal_rename,
            tauri_commands::terminal::terminal_attach,
            tauri_commands::terminal::claude_pure_refresh_title,
            tauri_commands::attachments::attachment_save,
            tauri_commands::badge::set_taskbar_badge,
            tauri_commands::action::action_list,
            tauri_commands::action::action_upsert,
            tauri_commands::action::action_delete,
            tauri_commands::action::action_run,
            tauri_commands::env::env_template_for_worktree,
            tauri_commands::env::env_status_for_worktree,
            tauri_commands::env::env_up_for_worktree,
            tauri_commands::env::env_down_for_worktree,
            tauri_commands::env::env_dotenv_render_for_worktree,
            tauri_commands::env::env_dotenv_status_for_worktree,
            tauri_commands::fs::fs_list_dir,
            tauri_commands::fs::fs_read_file,
            tauri_commands::fs::fs_write_file,
            tauri_commands::fs::fs_open_external,
            tauri_commands::fs::fs_external_editors,
            tauri_commands::fs::fs_read_file_bytes,
            tauri_commands::fs::fs_create_file,
            tauri_commands::fs::fs_create_dir,
            tauri_commands::fs::fs_rename,
            tauri_commands::fs::fs_delete,
            tauri_commands::fs::fs_copy,
            tauri_commands::fs::fs_abs_path,
            tauri_commands::fs::fs_reveal,
            tauri_commands::fs::fs_search_paths,
            tauri_commands::fs::fs_search_content,
            tauri_commands::git::git_status,
            tauri_commands::git::git_diff_file,
            tauri_commands::git::git_stage,
            tauri_commands::git::git_unstage,
            tauri_commands::git::git_commit,
            tauri_commands::git::git_fetch,
            tauri_commands::git::git_pull,
            tauri_commands::git::git_push,
            tauri_commands::git::git_checkout,
            tauri_commands::git::git_branch_create,
            tauri_commands::git::git_branch_delete,
            tauri_commands::git::git_log,
            tauri_commands::git::git_get_conflict,
            tauri_commands::git::git_resolve,
            tauri_commands::git::git_apply_patch,
            tauri_commands::git::git_generate_commit_message,
            tauri_commands::git::git_stash_list,
            tauri_commands::git::git_stash_save,
            tauri_commands::git::git_stash_apply,
            tauri_commands::git::git_stash_drop,
            tauri_commands::git::git_tag_list,
            tauri_commands::git::git_tag_create,
            tauri_commands::git::git_tag_delete,
            tauri_commands::git::git_cherry_pick,
            tauri_commands::git::git_revert,
            tauri_commands::git::git_diff_revs,
            tauri_commands::indexing::index_rebuild,
            tauri_commands::indexing::index_query_symbol,
            tauri_commands::indexing::index_list_symbols_in_file,
            tauri_commands::indexing::index_project_map,
            tauri_commands::indexing::index_stats,
            tauri_commands::indexing::worktree_ensure_ready,
            tauri_commands::language_packs::language_packs_list,
            tauri_commands::language_packs::language_packs_install,
            tauri_commands::language_packs::language_packs_uninstall,
            tauri_commands::language_packs::language_packs_install_in_wsl,
            tauri_commands::language_packs::wsl_distros,
        ])
        .setup(|app| {
            // Keep dev runs isolated from the installed release. Both share
            // the same `identifier` so without a suffix Tauri resolves both
            // to `%APPDATA%\dev.oxyris.app` and the installed app would see
            // events.sqlite from local cargo runs.
            #[allow(unused_mut)]
            let mut data_dir = app.path().app_data_dir().expect("resolve app_data_dir");
            #[cfg(debug_assertions)]
            {
                let leaf = data_dir
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "oxyris".into());
                data_dir.set_file_name(format!("{leaf}-dev"));
            }
            let state = AppState::initialize(app.handle().clone(), data_dir)?;
            app.manage(state);
            tracing::info!("oxyris-desktop v{} booted", env!("CARGO_PKG_VERSION"));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Oxyris");
}
