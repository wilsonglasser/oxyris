//! Spawns the actual `oxyris-mcp` binary as a child process and drives the
//! protocol over stdio with a populated SQLite index. Validates the full
//! path Claude will exercise — protocol framing, tool dispatch, content
//! shape — without needing Claude itself in the loop.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use oxyris_index::{Index, Lang};

fn populate_index(db_path: &std::path::Path) {
    let index = Index::open(db_path).expect("open index");
    index
        .index_file(
            "src/lib.rs",
            Lang::Rust,
            1,
            r#"
pub fn add(a: i32, b: i32) -> i32 { a + b }
pub struct UserService { name: String }
impl UserService {
    pub fn new(name: String) -> Self { Self { name } }
    pub fn greet(&self) -> String { format!("hi {}", self.name) }
}
pub trait Speak { fn say(&self); }
"#,
        )
        .expect("index sample");
}

fn binary_path() -> std::path::PathBuf {
    // The binary lives next to whatever profile cargo just built. CARGO_BIN_EXE_<name>
    // is provided by Cargo to integration tests in the same package.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_oxyris-mcp"))
}

#[test]
fn full_protocol_roundtrip_with_populated_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("index.db");
    populate_index(&db);

    let mut child = Command::new(binary_path())
        .arg("--index-db")
        .arg(&db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oxyris-mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut out = BufReader::new(stdout);

    // Initialize handshake.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
    )
    .unwrap();
    let init = read_response(&mut out);
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "oxyris-mcp");

    // Tools list.
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    let list = read_response(&mut out);
    let tools = list["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 3);

    // tools/call → oxyris_find_symbol("UserService") should locate it.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"oxyris_find_symbol","arguments":{{"name":"UserService"}}}}}}"#
    )
    .unwrap();
    let call = read_response(&mut out);
    let text = call["result"]["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("UserService"), "got: {text}");
    assert!(text.contains("src/lib.rs"), "got: {text}");

    // tools/call → oxyris_list_symbols on the file should include `add`,
    // `UserService`, and method names.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"oxyris_list_symbols","arguments":{{"file":"src/lib.rs"}}}}}}"#
    )
    .unwrap();
    let list_call = read_response(&mut out);
    let text = list_call["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert!(text.contains("add"), "got: {text}");
    assert!(text.contains("UserService"), "got: {text}");
    assert!(text.contains("greet"), "got: {text}");

    // tools/call → oxyris_project_map on a populated index reports counts.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"oxyris_project_map","arguments":{{}}}}}}"#
    )
    .unwrap();
    let map_call = read_response(&mut out);
    let text = map_call["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert!(text.contains("Project map"), "got: {text}");
    assert!(text.contains("src"), "got: {text}");

    // Closing stdin makes the server exit on next read.
    drop(stdin);
    let status = child.wait().expect("wait");
    assert!(status.success(), "exit status: {status:?}");
}

fn read_response(reader: &mut impl BufRead) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read line");
    serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("parse '{line}': {e}"))
}
