//! Parser for Claude CLI's `--output-format stream-json`.
//!
//! Claude emits one JSON object per line. The top-level `type` field
//! discriminates: `system`, `user`, `assistant`, `result`. Within `assistant`
//! and `user`, the `message.content[]` entries have their own `type`
//! (`text`, `thinking`, `tool_use`, `tool_result`).
//!
//! We flatten the nested shape into a flat [`StreamEvent`] enum so callers
//! don't need to mirror the CLI's internal structure.

use oxyris_provider::AssistantBlock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamEvent {
    System {
        session_id: Option<String>,
        model: Option<String>,
    },
    Assistant {
        blocks: Vec<AssistantBlock>,
    },
    /// Tool results come from claude as "user" messages — we surface them as
    /// their own event for clarity.
    ToolResult {
        tool_use_id: String,
        output: serde_json::Value,
        is_error: bool,
    },
    Result {
        is_error: bool,
        text: Option<String>,
        total_cost_usd: Option<f64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    /// Claude is asking permission to run a tool (`--permission-prompt-tool
    /// stdio`). We must reply with a `control_response` on stdin keyed by
    /// `request_id`, or the turn blocks forever.
    CanUseTool {
        request_id: String,
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    Unknown(serde_json::Value),
}

pub fn parse_stream_line(line: &str) -> Option<StreamEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            return Some(StreamEvent::Unknown(serde_json::Value::String(
                trimmed.to_owned(),
            )));
        }
    };

    let Some(type_str) = value.get("type").and_then(|t| t.as_str()) else {
        return Some(StreamEvent::Unknown(value));
    };

    match type_str {
        "system" => Some(StreamEvent::System {
            session_id: value
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            model: value
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        }),
        "assistant" => {
            let blocks = value
                .pointer("/message/content")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().filter_map(parse_content_block).collect())
                .unwrap_or_default();
            Some(StreamEvent::Assistant { blocks })
        }
        "user" => {
            // Tool results arrive as `user` messages with content[].type = "tool_result".
            if let Some(arr) = value.pointer("/message/content").and_then(|c| c.as_array()) {
                for item in arr {
                    if item.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                        let id = item
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        let output = item
                            .get("content")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        let is_error = item
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        return Some(StreamEvent::ToolResult {
                            tool_use_id: id.to_owned(),
                            output,
                            is_error,
                        });
                    }
                }
            }
            Some(StreamEvent::Unknown(value))
        }
        "result" => {
            let usage = value.get("usage");
            Some(StreamEvent::Result {
                is_error: value
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                text: value
                    .get("result")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                total_cost_usd: value.get("total_cost_usd").and_then(|v| v.as_f64()),
                input_tokens: usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|v| v.as_u64()),
                output_tokens: usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|v| v.as_u64()),
            })
        }
        "control_request" => {
            let request = value.get("request");
            let subtype = request
                .and_then(|r| r.get("subtype"))
                .and_then(|s| s.as_str());
            if subtype == Some("can_use_tool") {
                let request_id = value
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned();
                let request = request.unwrap_or(&serde_json::Value::Null);
                return Some(StreamEvent::CanUseTool {
                    request_id,
                    tool_use_id: request
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    tool_name: request
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    input: request
                        .get("input")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                });
            }
            // Other control subtypes (e.g. the response to our `initialize`)
            // aren't actionable here — keep them as Unknown.
            Some(StreamEvent::Unknown(value))
        }
        _ => Some(StreamEvent::Unknown(value)),
    }
}

fn parse_content_block(value: &serde_json::Value) -> Option<AssistantBlock> {
    let t = value.get("type").and_then(|v| v.as_str())?;
    match t {
        "text" => Some(AssistantBlock::Text {
            text: value
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
        }),
        "thinking" => Some(AssistantBlock::Thinking {
            text: value
                .get("thinking")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("text").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_owned(),
        }),
        "tool_use" => Some(AssistantBlock::ToolUse {
            id: value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            name: value
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            input: value
                .get("input")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_system_init_line() {
        let line =
            r#"{"type":"system","subtype":"init","session_id":"abc123","model":"claude-opus-4-7"}"#;
        let ev = parse_stream_line(line).unwrap();
        assert!(matches!(
            ev,
            StreamEvent::System { ref session_id, ref model }
                if session_id.as_deref() == Some("abc123") && model.as_deref() == Some("claude-opus-4-7")
        ));
    }

    #[test]
    fn parses_assistant_text_block() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#;
        let ev = parse_stream_line(line).unwrap();
        match ev {
            StreamEvent::Assistant { blocks } => {
                assert_eq!(blocks.len(), 1);
                assert!(matches!(&blocks[0], AssistantBlock::Text { text } if text == "hi"));
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn parses_tool_use_block() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"cmd":"ls"}}]}}"#;
        let ev = parse_stream_line(line).unwrap();
        match ev {
            StreamEvent::Assistant { blocks } => match &blocks[0] {
                AssistantBlock::ToolUse { id, name, input } => {
                    assert_eq!(id, "t1");
                    assert_eq!(name, "Bash");
                    assert_eq!(input["cmd"], "ls");
                }
                _ => panic!("wrong block"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn parses_tool_result_as_user_message() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}]}}"#;
        let ev = parse_stream_line(line).unwrap();
        assert!(matches!(
            ev,
            StreamEvent::ToolResult { ref tool_use_id, is_error: false, .. } if tool_use_id == "t1"
        ));
    }

    #[test]
    fn parses_result_with_usage() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","total_cost_usd":0.002,"usage":{"input_tokens":10,"output_tokens":5}}"#;
        let ev = parse_stream_line(line).unwrap();
        match ev {
            StreamEvent::Result {
                is_error,
                text,
                total_cost_usd,
                input_tokens,
                output_tokens,
            } => {
                assert!(!is_error);
                assert_eq!(text.as_deref(), Some("ok"));
                assert_eq!(total_cost_usd, Some(0.002));
                assert_eq!(input_tokens, Some(10));
                assert_eq!(output_tokens, Some(5));
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn parses_can_use_tool_control_request() {
        let line = r#"{"type":"control_request","request_id":"req-9","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"a.txt"},"tool_use_id":"toolu_1"}}"#;
        let ev = parse_stream_line(line).unwrap();
        match ev {
            StreamEvent::CanUseTool {
                request_id,
                tool_use_id,
                tool_name,
                input,
            } => {
                assert_eq!(request_id, "req-9");
                assert_eq!(tool_use_id, "toolu_1");
                assert_eq!(tool_name, "Write");
                assert_eq!(input["file_path"], "a.txt");
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn other_control_request_subtype_is_unknown() {
        let line =
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"init-1"}}"#;
        assert!(matches!(
            parse_stream_line(line).unwrap(),
            StreamEvent::Unknown(_)
        ));
    }

    #[test]
    fn unknown_type_survives_as_unknown() {
        let line = r#"{"type":"future_event","foo":1}"#;
        let ev = parse_stream_line(line).unwrap();
        assert!(matches!(ev, StreamEvent::Unknown(_)));
    }

    #[test]
    fn garbage_line_survives_as_unknown() {
        let ev = parse_stream_line("not json").unwrap();
        assert!(matches!(ev, StreamEvent::Unknown(_)));
    }
}
