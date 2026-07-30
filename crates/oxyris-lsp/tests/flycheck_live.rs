//! Live protocol test: spawn a *real* rust-analyzer over a throwaway Cargo
//! project and assert the check pipeline behaves.
//!
//! Ignored by default — it needs `rust-analyzer` and `cargo` installed and
//! takes tens of seconds, so it is not part of the normal gate. Run it after
//! touching the sync / flycheck / progress code:
//!
//! ```text
//! cargo test -p oxyris-lsp --test flycheck_live -- --ignored --nocapture
//! ```
//!
//! Unit tests cover the message shapes; only this covers the parts that depend
//! on how rust-analyzer actually behaves — that `rust-analyzer/runFlycheck` is
//! honoured, that its progress token is the one we wait on, and that a
//! `didChange`/`didSave` pushed for an edit made *behind the server's back*
//! makes the next check report disk truth.

use std::path::Path;
use std::time::Instant;

use oxyris_lsp::{LspClient, LspLanguage, resolve_server};

const CLEAN: &str = r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    println!("{}", add(1, 2));
}
"#;

/// Type error, not a parse error: a parse error would show up in
/// rust-analyzer's own analysis, so it would not prove the `cargo check` layer
/// ran. A wrong argument type only surfaces from a real check.
const BROKEN: &str = r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    println!("{}", add("one", 2));
}
"#;

fn errors_mentioning(
    all: &[(
        oxyris_lsp::lsp_types::Uri,
        Vec<oxyris_lsp::lsp_types::Diagnostic>,
    )],
    file_suffix: &str,
) -> Vec<String> {
    all.iter()
        .filter(|(uri, _)| uri.as_str().replace('\\', "/").ends_with(file_suffix))
        .flat_map(|(_, diags)| diags.iter())
        .filter(|d| {
            matches!(
                d.severity,
                Some(oxyris_lsp::lsp_types::DiagnosticSeverity::ERROR)
            )
        })
        .map(|d| d.message.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "spawns a real rust-analyzer and runs cargo check; slow, needs both on PATH"]
async fn check_reports_disk_truth_after_an_external_edit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"flycheck-probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    let main_rs = root.join("src").join("main.rs");
    std::fs::write(&main_rs, CLEAN).expect("write main.rs");

    let (binary, args) = resolve_server(LspLanguage::Rust).expect("rust-analyzer on PATH");
    let started = Instant::now();
    let client = LspClient::spawn(
        &binary,
        &args,
        root,
        LspLanguage::Rust.initialization_options(),
    )
    .await
    .expect("spawn rust-analyzer");
    eprintln!("spawned in {:?}", started.elapsed());

    // Baseline: a clean project must produce a completed check and no errors.
    assert!(
        client.sync_from_disk(&main_rs).await.expect("open"),
        "first sync opens the document"
    );
    let ran = client
        .run_check_and_wait(None)
        .await
        .expect("flycheck settles");
    assert!(
        ran,
        "rust-analyzer must honour runFlycheck on a Cargo project"
    );
    let clean = client.all_diagnostics().await;
    assert!(
        errors_mentioning(&clean, "src/main.rs").is_empty(),
        "clean project reported errors: {clean:?}"
    );

    // Now the case the whole change exists for: the file is edited by someone
    // who is not this client (our agent's file tools), so the server's buffer
    // is stale until we push the change.
    std::fs::write(&main_rs, BROKEN).expect("rewrite main.rs");
    assert!(
        client.sync_from_disk(&main_rs).await.expect("sync edit"),
        "an on-disk edit must be detected and pushed"
    );
    assert!(
        !client.sync_from_disk(&main_rs).await.expect("re-sync"),
        "an unchanged file must not be pushed again"
    );

    let ran = client
        .run_check_and_wait(None)
        .await
        .expect("flycheck settles");
    assert!(ran, "second check must run");
    let broken = client.all_diagnostics().await;
    let errors = errors_mentioning(&broken, "src/main.rs");
    assert!(
        !errors.is_empty(),
        "the type error must be reported after the external edit: {broken:?}"
    );
    assert!(
        errors.iter().any(|m| m.contains("mismatched types")),
        "expected a type mismatch, got {errors:?}"
    );

    // And back: fixing the file must clear it, so a caller can trust "clean"
    // as much as it trusts "broken".
    std::fs::write(&main_rs, CLEAN).expect("restore main.rs");
    client.sync_from_disk(&main_rs).await.expect("sync fix");
    client
        .run_check_and_wait(None)
        .await
        .expect("flycheck settles");
    let fixed = client.all_diagnostics().await;
    assert!(
        errors_mentioning(&fixed, "src/main.rs").is_empty(),
        "errors survived the fix: {fixed:?}"
    );

    eprintln!("total {:?}", started.elapsed());
    client.shutdown().await;
}

/// A non-Cargo directory has no check layer, so the wait must give up quickly
/// instead of blocking a tool call for the full flycheck timeout.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "spawns a real rust-analyzer; slow"]
async fn wait_gives_up_when_no_check_can_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, args) = resolve_server(LspLanguage::Rust).expect("rust-analyzer on PATH");
    let client = LspClient::spawn(
        &binary,
        &args,
        dir.path(),
        LspLanguage::Rust.initialization_options(),
    )
    .await
    .expect("spawn rust-analyzer");

    let started = Instant::now();
    let ran = client.run_check_and_wait(None).await.expect("no error");
    assert!(!ran, "there is no Cargo project here to check");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "gave up too slowly: {:?}",
        started.elapsed()
    );
    client.shutdown().await;
}

/// Guard the assumption the whole design leans on: `Path` is not enough, the
/// URI the server publishes against has to round-trip back to the file we asked
/// about, or workspace-wide reports would be unattributable.
#[test]
fn probe_paths_are_absolute() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p: &Path = dir.path();
    assert!(p.is_absolute(), "tempdir must be absolute: {}", p.display());
}
