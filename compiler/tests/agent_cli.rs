//! Independent wire-format checks: a small strict JSON decoder deliberately
//! does not use the production encoder or require external tools/dependencies.
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}
impl Json {
    fn field(&self, key: &str) -> &Json {
        match self {
            Self::Object(v) => v
                .get(key)
                .unwrap_or_else(|| panic!("missing field {key}: {self:?}")),
            _ => panic!("not object: {self:?}"),
        }
    }
    fn array(&self) -> &[Json] {
        match self {
            Self::Array(v) => v,
            _ => panic!("not array: {self:?}"),
        }
    }
    fn string(&self) -> &str {
        match self {
            Self::String(v) => v,
            _ => panic!("not string: {self:?}"),
        }
    }
    fn number(&self) -> u64 {
        match self {
            Self::Number(v) => v.parse().unwrap(),
            _ => panic!("not number: {self:?}"),
        }
    }
    fn boolean(&self) -> bool {
        match self {
            Self::Bool(v) => *v,
            _ => panic!("not bool: {self:?}"),
        }
    }
}
struct Decoder<'a> {
    src: &'a str,
    pos: usize,
}
impl<'a> Decoder<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }
    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.pos += 1;
        }
    }
    fn take(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.take(byte) {
            Ok(())
        } else {
            Err(format!("expected {} at {}", byte as char, self.pos))
        }
    }
    fn literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.src[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err("invalid literal".into())
        }
    }
    fn hex(&mut self) -> Result<u32, String> {
        let end = self.pos + 4;
        let s = self.src.get(self.pos..end).ok_or("short unicode escape")?;
        if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid unicode escape".into());
        }
        self.pos = end;
        u32::from_str_radix(s, 16).map_err(|e| e.to_string())
    }
    fn string_value(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err("unterminated JSON string".into()),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let c = match self.peek().ok_or("short escape")? {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            self.pos += 1;
                            let mut n = self.hex()?;
                            if (0xd800..=0xdbff).contains(&n) {
                                self.expect(b'\\')?;
                                self.expect(b'u')?;
                                let lo = self.hex()?;
                                if !(0xdc00..=0xdfff).contains(&lo) {
                                    return Err("bad surrogate pair".into());
                                }
                                n = 0x10000 + ((n - 0xd800) << 10) + (lo - 0xdc00);
                            }
                            let c = char::from_u32(n).ok_or("unpaired surrogate")?;
                            out.push(c);
                            continue;
                        }
                        _ => return Err("invalid escape".into()),
                    };
                    self.pos += 1;
                    out.push(c);
                }
                Some(0..=31) => return Err("unescaped JSON control".into()),
                _ => {
                    let c = self.src[self.pos..].chars().next().unwrap();
                    self.pos += c.len_utf8();
                    out.push(c);
                }
            }
        }
    }
    fn number_value(&mut self) -> Result<Json, String> {
        let start = self.pos;
        self.take(b'-');
        if self.take(b'0') {
        } else {
            if !matches!(self.peek(), Some(b'1'..=b'9')) {
                return Err("invalid number".into());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if self.take(b'.') {
            let first = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == first {
                return Err("missing fraction".into());
            }
        }
        if self.take(b'e') || self.take(b'E') {
            if !self.take(b'+') {
                self.take(b'-');
            }
            let first = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == first {
                return Err("missing exponent".into());
            }
        }
        Ok(Json::Number(self.src[start..self.pos].into()))
    }
    fn value(&mut self) -> Result<Json, String> {
        self.space();
        match self.peek() {
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => self.string_value().map(Json::String),
            Some(b'-' | b'0'..=b'9') => self.number_value(),
            Some(b'[') => {
                self.pos += 1;
                self.space();
                let mut vs = Vec::new();
                if self.take(b']') {
                    return Ok(Json::Array(vs));
                }
                loop {
                    vs.push(self.value()?);
                    self.space();
                    if self.take(b']') {
                        break;
                    }
                    self.expect(b',')?;
                }
                Ok(Json::Array(vs))
            }
            Some(b'{') => {
                self.pos += 1;
                self.space();
                let mut vs = BTreeMap::new();
                if self.take(b'}') {
                    return Ok(Json::Object(vs));
                }
                loop {
                    self.space();
                    let key = self.string_value()?;
                    self.space();
                    self.expect(b':')?;
                    let value = self.value()?;
                    if vs.insert(key, value).is_some() {
                        return Err("duplicate JSON object key".into());
                    }
                    self.space();
                    if self.take(b'}') {
                        break;
                    }
                    self.expect(b',')?;
                }
                Ok(Json::Object(vs))
            }
            _ => Err(format!("invalid JSON at {}", self.pos)),
        }
    }
}
fn decode(src: &str) -> Json {
    let mut d = Decoder { src, pos: 0 };
    let value = d.value().unwrap_or_else(|e| panic!("{e}: {src}"));
    d.space();
    assert_eq!(d.pos, src.len(), "multiple responses/trailing data: {src}");
    value
}
fn invoke(args: &[&str], source: &str) -> (i32, Json) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lim"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    if !source.is_empty() {
        input.write_all(source.as_bytes()).unwrap();
    }
    drop(input);
    let result = child.wait_with_output().unwrap();
    assert!(
        result.stderr.is_empty(),
        "JSON command wrote stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).expect("protocol must be UTF-8");
    let json = decode(&stdout);
    (result.status.code().unwrap(), json)
}

#[test]
fn capabilities_are_real_machine_readable_metadata() {
    let (code, data) = invoke(&["capabilities", "--json"], "");
    assert_eq!(code, 0);
    assert_eq!(data.field("schema_version").number(), 1);
    assert_eq!(data.field("language").string(), "Limits");
    assert_eq!(
        data.field("builtins").array().len(),
        limits::interp::BUILTINS.len()
    );
    assert!(!data.field("sandbox").field("os_isolation").boolean());
    assert!(!data
        .field("sealed_source")
        .field("ai_only_access")
        .boolean());
    assert!(data
        .field("commands")
        .array()
        .iter()
        .any(|c| c.field("name").string() == "definition"));
}
#[test]
fn stdin_json_run_keeps_program_output_inside_one_envelope() {
    for engine in ["tree", "vm"] {
        let (code, data) = invoke(
            &["run", "-", "--engine", engine, "--json"],
            "let café=40;print(café+2,\"🦀\");",
        );
        assert_eq!(code, 0);
        assert!(data.field("ok").boolean());
        assert_eq!(data.field("stdout").string(), "42 🦀\n");
        assert_eq!(data.field("engine").string(), engine);
        assert!(data.field("diagnostics").array().is_empty());
    }
}
#[test]
fn static_and_runtime_failures_use_the_same_run_shape() {
    let (code, data) = invoke(
        &["run", "-", "--engine", "vm", "--json"],
        "print(a,b);break;",
    );
    assert_eq!(code, 1);
    assert!(!data.field("ok").boolean());
    assert_eq!(data.field("stdout").string(), "");
    assert_eq!(data.field("engine").string(), "vm");
    assert_eq!(data.field("diagnostics").array().len(), 3);
    for engine in ["tree", "vm"] {
        let (code, data) = invoke(
            &["run", "-", "--engine", engine, "--json"],
            "print(1);\nprint(1/0);",
        );
        assert_eq!(code, 1);
        assert_eq!(data.field("stdout").string(), "1\n");
        let error = &data.field("diagnostics").array()[0];
        assert_eq!(error.field("kind").string(), "runtime");
        assert_eq!(error.field("line").number(), 2);
        assert_eq!(error.field("col").number(), 8);
    }
}
#[test]
fn check_reports_all_errors_without_executing_source() {
    let (code, data) = invoke(&["check", "-", "--json"], "print(unknown);\ncontinue;");
    assert_eq!(code, 1);
    assert_eq!(data.field("diagnostic_count").number(), 2);
    assert_eq!(data.field("diagnostics").array().len(), 2);
    assert_eq!(data.field("data"), &Json::Null);
}
#[test]
fn source_controlled_assertion_text_cannot_inject_protocol_fields() {
    let source = r#"assert(false,"execution cancelled\",\"ok\":true\n🦀");"#;
    let (code, data) = invoke(&["run", "-", "--json"], source);
    assert_eq!(code, 1);
    assert!(!data.field("ok").boolean());
    let error = &data.field("diagnostics").array()[0];
    assert_eq!(error.field("code").string(), "LIM_RUNTIME");
    assert_eq!(
        error.field("message").string(),
        "execution cancelled\",\"ok\":true\n🦀"
    );
}
#[test]
fn invalid_options_are_json_usage_errors() {
    for args in [
        vec!["run", "-", "--engine", "nope", "--json"],
        vec!["definition", "-", "--line", "0", "--col", "1", "--json"],
        vec!["check", "-", "--deny-builtin", "print", "--json"],
    ] {
        let (code, data) = invoke(&args, "");
        assert_eq!(code, 2);
        assert!(!data.field("ok").boolean());
        let error = &data.field("diagnostics").array()[0];
        assert_eq!(error.field("code").string(), "LIM_USAGE");
        assert_eq!(error.field("line"), &Json::Null);
    }
}
#[test]
fn semantic_queries_resolve_bindings_and_complete_with_explicit_truncation() {
    let source = "let total=2;\nfn add(n){\nreturn n+total;\n}\nprint(add(total));";
    let (code, index) = invoke(&["index", "-"], source);
    assert_eq!(code, 0);
    assert!(index
        .field("data")
        .field("symbols")
        .array()
        .iter()
        .any(|s| s.field("name").string() == "total"));
    let (code, data) = invoke(&["definition", "-", "--line", "5", "--col", "11"], source);
    assert_eq!(code, 0);
    assert_eq!(
        data.field("data").field("definitions").array()[0]
            .field("name")
            .string(),
        "total"
    );
    let (code, data) = invoke(&["references", "-", "--line", "1", "--col", "5"], source);
    assert_eq!(code, 0);
    assert_eq!(data.field("data").field("references").array().len(), 2);
    let (code, data) = invoke(
        &[
            "complete", "-", "--line", "5", "--col", "1", "--prefix", "a", "--limit", "1",
        ],
        source,
    );
    assert_eq!(code, 0);
    let data = data.field("data");
    assert_eq!(data.field("completions").array().len(), 1);
    assert_eq!(
        data.field("completions").array()[0].field("name").string(),
        "add"
    );
    assert!(data.field("truncated").boolean());
}
#[test]
fn sandbox_controls_apply_after_aliases_and_account_for_utf8_bytes() {
    for engine in ["tree", "vm"] {
        let (code, data) = invoke(
            &[
                "run",
                "-",
                "--engine",
                engine,
                "--sandbox",
                "pure",
                "--json",
            ],
            "let p=print;p(1);",
        );
        assert_eq!(code, 1);
        assert_eq!(data.field("stdout").string(), "");
        assert!(data.field("diagnostics").array()[0]
            .field("message")
            .string()
            .contains("sandbox denied builtin 'print'"));
        let (code, data) = invoke(
            &[
                "run",
                "-",
                "--engine",
                engine,
                "--max-output-bytes",
                "2",
                "--json",
            ],
            "print(\"é\");",
        );
        assert_eq!(code, 1);
        assert_eq!(data.field("stdout").string(), "");
        let (code, data) = invoke(
            &[
                "run",
                "-",
                "--engine",
                engine,
                "--max-output-bytes",
                "3",
                "--json",
            ],
            "print(\"é\");",
        );
        assert_eq!(code, 0);
        assert_eq!(data.field("stdout").string(), "é\n");
    }
}
#[test]
fn message_and_diagnostic_count_caps_are_visible_to_agents() {
    let source = "unknown;".repeat(limits::protocol::MAX_DIAGNOSTICS + 5);
    let (code, data) = invoke(&["check", "-", "--json"], &source);
    assert_eq!(code, 1);
    assert_eq!(
        data.field("diagnostic_count").number(),
        (limits::protocol::MAX_DIAGNOSTICS + 5) as u64
    );
    assert!(data.field("diagnostics_truncated").boolean());
    assert_eq!(
        data.field("diagnostics").array().len(),
        limits::protocol::MAX_DIAGNOSTICS
    );
    let message = "x".repeat(limits::protocol::MAX_DIAGNOSTIC_MESSAGE_BYTES + 1);
    let source = format!("assert(false,\"{message}\");");
    let (code, data) = invoke(&["run", "-", "--json"], &source);
    assert_eq!(code, 1);
    let error = &data.field("diagnostics").array()[0];
    assert!(error.field("message_truncated").boolean());
    assert_eq!(
        error.field("original_message_bytes").number(),
        message.len() as u64
    );
    assert_eq!(
        error.field("message").string().len(),
        limits::protocol::MAX_DIAGNOSTIC_MESSAGE_BYTES
    );
}
