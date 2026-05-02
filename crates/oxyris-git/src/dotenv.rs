//! Pure dotenv merge / substitute / render — used by the per-worktree env
//! generator. Independent of any IO so both desktop and agent can call into
//! the same logic.
//!
//! The flow is: `parse(base) → parse(overlay) → merge → substitute(vars) →
//! render`. Lines from the base preserve their order and any comments around
//! them; overlay-only keys land at the end. Substitution understands
//! `${VAR}` and `${VAR:-fallback}`.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DotenvLine {
    /// `KEY=value` (or `KEY=` for empty). `raw_value` is the right-hand side
    /// exactly as the user wrote it (with quotes if any). We re-render the
    /// original syntax so existing files stay diff-friendly.
    KeyValue {
        key: String,
        raw_value: String,
        export_prefix: bool,
    },
    Comment(String),
    Blank,
}

#[derive(Debug, Default)]
pub struct Dotenv {
    pub lines: Vec<DotenvLine>,
}

impl Dotenv {
    pub fn parse(content: &str) -> Self {
        let mut out = Vec::new();
        for raw in content.lines() {
            let line = raw.trim_start();
            if line.is_empty() {
                out.push(DotenvLine::Blank);
                continue;
            }
            if line.starts_with('#') {
                out.push(DotenvLine::Comment(raw.to_owned()));
                continue;
            }
            // Optional `export ` prefix.
            let (export_prefix, rest) = if let Some(stripped) = line.strip_prefix("export ") {
                (true, stripped)
            } else {
                (false, line)
            };
            let Some(eq_pos) = rest.find('=') else {
                // Malformed — keep as comment so we don't lose data.
                out.push(DotenvLine::Comment(raw.to_owned()));
                continue;
            };
            let key = rest[..eq_pos].trim().to_owned();
            if key.is_empty() || !key.chars().next().is_some_and(valid_key_start) {
                out.push(DotenvLine::Comment(raw.to_owned()));
                continue;
            }
            let raw_value = rest[eq_pos + 1..].to_owned();
            out.push(DotenvLine::KeyValue {
                key,
                raw_value,
                export_prefix,
            });
        }
        Dotenv { lines: out }
    }

    /// Lookup a key's raw value (post-parse, pre-substitution).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines.iter().find_map(|l| match l {
            DotenvLine::KeyValue {
                key: k, raw_value, ..
            } if k == key => Some(raw_value.as_str()),
            _ => None,
        })
    }

    /// Set / overwrite a key. Used internally by `merge`; exposed so
    /// callers can pre-populate a base before merging.
    pub fn set(&mut self, key: &str, raw_value: String) {
        for line in self.lines.iter_mut() {
            if let DotenvLine::KeyValue {
                key: k,
                raw_value: v,
                ..
            } = line
                && k == key
            {
                *v = raw_value;
                return;
            }
        }
        self.lines.push(DotenvLine::KeyValue {
            key: key.to_owned(),
            raw_value,
            export_prefix: false,
        });
    }

    /// Merge `overlay` on top of `self` — overlay wins on key conflict, and
    /// any keys present only in overlay are appended at the end (preserving
    /// the comments around them).
    pub fn merge(&mut self, overlay: &Dotenv) {
        // Collect keys that the overlay defines, in order, so we can decide
        // which lines to copy as new (only overlay-only keys + their leading
        // comments).
        let mut overlay_only_keys: Vec<String> = Vec::new();
        for line in &overlay.lines {
            if let DotenvLine::KeyValue { key, raw_value, .. } = line {
                if self.contains_key(key) {
                    self.set(key, raw_value.clone());
                } else {
                    overlay_only_keys.push(key.clone());
                }
            }
        }
        if overlay_only_keys.is_empty() {
            return;
        }
        // Append a separator comment + the overlay-only keys so it's obvious
        // where they came from.
        if !matches!(self.lines.last(), Some(DotenvLine::Blank) | None) {
            self.lines.push(DotenvLine::Blank);
        }
        let mut pending_comments: Vec<DotenvLine> = Vec::new();
        for line in &overlay.lines {
            match line {
                DotenvLine::Comment(_) => pending_comments.push(line.clone()),
                DotenvLine::Blank => pending_comments.clear(),
                DotenvLine::KeyValue { key, .. } if overlay_only_keys.contains(key) => {
                    self.lines.append(&mut pending_comments);
                    self.lines.push(line.clone());
                }
                _ => {
                    pending_comments.clear();
                }
            }
        }
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Substitute `${VAR}` and `${VAR:-fallback}` references inside every
    /// value (preserving quotes). Unknown vars without a fallback are left
    /// literal so the user can spot them.
    pub fn substitute(&mut self, vars: &HashMap<String, String>) {
        for line in self.lines.iter_mut() {
            if let DotenvLine::KeyValue { raw_value, .. } = line {
                *raw_value = substitute_in(raw_value, vars);
            }
        }
    }

    /// Render back to a `.env`-shaped string.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match line {
                DotenvLine::Blank => out.push('\n'),
                DotenvLine::Comment(c) => {
                    out.push_str(c);
                    out.push('\n');
                }
                DotenvLine::KeyValue {
                    key,
                    raw_value,
                    export_prefix,
                } => {
                    if *export_prefix {
                        out.push_str("export ");
                    }
                    out.push_str(key);
                    out.push('=');
                    out.push_str(raw_value);
                    out.push('\n');
                }
            }
        }
        out
    }
}

fn valid_key_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn substitute_in(input: &str, vars: &HashMap<String, String>) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len()
            && bytes[i] == b'$'
            && bytes[i + 1] == b'{'
            && let Some(end) = find_matching_brace(&bytes[i + 2..])
        {
            let token = &input[i + 2..i + 2 + end];
            let replacement = resolve_token(token, vars);
            out.push_str(&replacement);
            i += 2 + end + 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn find_matching_brace(slice: &[u8]) -> Option<usize> {
    slice.iter().position(|b| *b == b'}')
}

fn resolve_token(token: &str, vars: &HashMap<String, String>) -> String {
    if let Some(idx) = token.find(":-") {
        let name = &token[..idx];
        let fallback = &token[idx + 2..];
        match vars.get(name) {
            Some(v) if !v.is_empty() => v.clone(),
            _ => fallback.to_owned(),
        }
    } else {
        match vars.get(token) {
            Some(v) => v.clone(),
            None => format!("${{{token}}}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> HashMap<String, String> {
        HashMap::from([
            ("OXYRIS_WORKTREE_SHORT".into(), "a3f12bc8".into()),
            ("OXYRIS_PORT_OFFSET".into(), "731".into()),
        ])
    }

    #[test]
    fn parse_and_render_roundtrips_simple() {
        let src = "FOO=bar\n# comment\nBAZ=qux\n";
        let env = Dotenv::parse(src);
        assert_eq!(env.render(), src);
    }

    #[test]
    fn merge_overlay_wins_on_conflict() {
        let mut base = Dotenv::parse("PORT=3000\nDB=local\n");
        let overlay = Dotenv::parse("PORT=4000\n");
        base.merge(&overlay);
        assert_eq!(base.get("PORT"), Some("4000"));
        assert_eq!(base.get("DB"), Some("local"));
    }

    #[test]
    fn merge_appends_overlay_only_keys() {
        let mut base = Dotenv::parse("FOO=1\n");
        let overlay = Dotenv::parse("BAR=2\n");
        base.merge(&overlay);
        assert_eq!(base.get("FOO"), Some("1"));
        assert_eq!(base.get("BAR"), Some("2"));
    }

    #[test]
    fn substitute_resolves_variables_and_fallback() {
        let mut env = Dotenv::parse(
            "URL=postgres://localhost:${OXYRIS_PORT_OFFSET:-5432}/app_${OXYRIS_WORKTREE_SHORT}\nNOPE=${UNKNOWN:-default}\n",
        );
        env.substitute(&vars());
        assert_eq!(
            env.get("URL"),
            Some("postgres://localhost:731/app_a3f12bc8")
        );
        assert_eq!(env.get("NOPE"), Some("default"));
    }

    #[test]
    fn substitute_leaves_unknown_without_fallback_literal() {
        let mut env = Dotenv::parse("X=${UNKNOWN}\n");
        env.substitute(&vars());
        assert_eq!(env.get("X"), Some("${UNKNOWN}"));
    }

    #[test]
    fn export_prefix_preserved() {
        let env = Dotenv::parse("export FOO=bar\n");
        assert_eq!(env.render(), "export FOO=bar\n");
    }

    #[test]
    fn full_merge_substitute_pipeline() {
        let base =
            "DATABASE_URL=postgres://localhost:5432/app\nREDIS=redis://localhost:6379\nLOG=debug\n";
        let template = "DATABASE_URL=postgres://localhost:5432/app_${OXYRIS_WORKTREE_SHORT}\nPORT=${OXYRIS_PORT_OFFSET:-3000}\n";
        let mut env = Dotenv::parse(base);
        let overlay = Dotenv::parse(template);
        env.merge(&overlay);
        env.substitute(&vars());
        assert_eq!(
            env.get("DATABASE_URL"),
            Some("postgres://localhost:5432/app_a3f12bc8")
        );
        assert_eq!(env.get("REDIS"), Some("redis://localhost:6379"));
        assert_eq!(env.get("LOG"), Some("debug"));
        assert_eq!(env.get("PORT"), Some("731"));
    }
}
