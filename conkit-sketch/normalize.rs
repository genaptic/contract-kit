#[derive(Debug, Eq, PartialEq)]
pub(crate) struct NormalizedSnippet {
    lines: Vec<NormalizedLine>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NormalizedLine {
    bytes: Vec<u8>,
}

impl NormalizedSnippet {
    pub(crate) fn from_code(code: &str) -> Self {
        Self::from_bytes(code.as_bytes())
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        let lines = bytes
            .split(|byte| *byte == b'\n')
            .map(NormalizedLine::from_raw_bytes)
            .filter(|line| !line.is_empty())
            .collect();

        Self { lines }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub(crate) fn occurs_in(&self, source: &Self) -> bool {
        !self.is_empty()
            && source
                .lines
                .windows(self.lines.len())
                .any(|window| window == self.lines.as_slice())
    }
}

impl NormalizedLine {
    fn from_raw_bytes(raw: &[u8]) -> Self {
        match std::str::from_utf8(raw) {
            Ok(text) => NormalizedLineBuilder::from_utf8(text).finish(),
            Err(_) => NormalizedLineBuilder::from_non_utf8(raw).finish(),
        }
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

struct NormalizedLineBuilder {
    bytes: Vec<u8>,
    pending_space: bool,
}

impl NormalizedLineBuilder {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            pending_space: false,
        }
    }

    fn from_utf8(text: &str) -> Self {
        let mut builder = Self::new();

        for ch in text.chars() {
            if ch.is_whitespace() {
                builder.pending_space = true;
            } else {
                let mut buffer = [0; 4];
                builder.push_token(ch.encode_utf8(&mut buffer).as_bytes());
            }
        }

        builder
    }

    fn from_non_utf8(raw: &[u8]) -> Self {
        let mut builder = Self::new();

        for byte in raw {
            if byte.is_ascii_whitespace() {
                builder.pending_space = true;
            } else {
                builder.push_token(std::slice::from_ref(byte));
            }
        }

        builder
    }

    fn push_token(&mut self, token: &[u8]) {
        if self.pending_space && !self.bytes.is_empty() {
            self.bytes.push(b' ');
        }

        self.pending_space = false;
        self.bytes.extend_from_slice(token);
    }

    fn finish(self) -> NormalizedLine {
        NormalizedLine { bytes: self.bytes }
    }
}

#[cfg(test)]
mod tests {
    use super::{NormalizedLine, NormalizedSnippet};

    #[test]
    fn line_normalization_trims_and_collapses_whitespace() {
        let line = NormalizedLine::from_raw_bytes(b" \tlet   value\t=\tinput.trim();  ");

        assert_eq!(line.bytes, b"let value = input.trim();");
    }

    #[test]
    fn line_normalization_preserves_non_whitespace_tokens_as_utf8() {
        let line = NormalizedLine::from_raw_bytes(" let name = \"Ω\"; ".as_bytes());

        assert_eq!(line.bytes, "let name = \"Ω\";".as_bytes());
    }

    #[test]
    fn byte_normalization_preserves_non_utf8_token_bytes() {
        let snippet = NormalizedSnippet::from_bytes(b"let bytes = \xFF;\n");

        assert!(!snippet.is_empty());
        assert!(snippet.occurs_in(&NormalizedSnippet::from_bytes(b"let   bytes = \xFF;\n")));
    }

    #[test]
    fn unicode_whitespace_normalizes_to_ascii_space() {
        for whitespace in ['\u{00a0}', '\u{2003}'] {
            let expected = NormalizedSnippet::from_code("let value = 42;");
            let source = NormalizedSnippet::from_code(&format!(
                "{whitespace}let{whitespace}value{whitespace}={whitespace}42;{whitespace}"
            ));

            assert!(
                expected.occurs_in(&source),
                "Unicode whitespace {whitespace:?} should normalize"
            );
        }
    }

    #[test]
    fn snippet_normalization_drops_blank_lines_and_preserves_order() {
        let snippet = NormalizedSnippet::from_code(
            "\n\
                 fn answer() -> u8 {\n\
                 \n\
                     42\n\
                 }\n",
        );

        assert_eq!(snippet.lines.len(), 3);
    }

    #[test]
    fn crlf_and_lf_normalize_the_same() {
        let crlf = NormalizedSnippet::from_code("fn answer() {\r\n    todo!()\r\n}\r\n");
        let lf = NormalizedSnippet::from_code("fn answer() {\n    todo!()\n}\n");

        assert!(crlf.occurs_in(&lf));
        assert!(lf.occurs_in(&crlf));
    }

    #[test]
    fn whitespace_differences_do_not_prevent_match() {
        let expected = NormalizedSnippet::from_code(
            "fn answer() -> u8 {\n    let value   =  42;\n    value\n}",
        );
        let source = NormalizedSnippet::from_code(
            "mod inner {\n\tfn answer() -> u8 {\n\t\tlet   value = 42;\n\t\tvalue\n\t}\n}",
        );

        assert!(expected.occurs_in(&source));
    }

    #[test]
    fn empty_snippet_never_matches() {
        let expected = NormalizedSnippet::from_code("\n   \n\t");
        let source = NormalizedSnippet::from_code("fn answer() -> u8 { 42 }");

        assert!(expected.is_empty());
        assert!(!expected.occurs_in(&source));
    }

    #[test]
    fn changed_tokens_fail_match() {
        let expected = NormalizedSnippet::from_code("let value = 42;");
        let source = NormalizedSnippet::from_code("let value = 41;");

        assert!(!expected.occurs_in(&source));
    }

    #[test]
    fn missing_lines_fail_match() {
        let expected = NormalizedSnippet::from_code("fn answer() {\nlet value = 42;\nvalue\n}");
        let source = NormalizedSnippet::from_code("fn answer() {\nvalue\n}");

        assert!(!expected.occurs_in(&source));
    }

    #[test]
    fn reordered_lines_fail_match() {
        let expected = NormalizedSnippet::from_code("let first = 1;\nlet second = 2;");
        let source = NormalizedSnippet::from_code("let second = 2;\nlet first = 1;");

        assert!(!expected.occurs_in(&source));
    }

    #[test]
    fn extra_lines_inside_candidate_window_fail_match() {
        let expected = NormalizedSnippet::from_code("let first = 1;\nlet second = 2;");
        let source =
            NormalizedSnippet::from_code("let first = 1;\nlet inserted = 99;\nlet second = 2;");

        assert!(!expected.occurs_in(&source));
    }

    #[test]
    fn source_can_contain_snippet_inside_larger_file() {
        let expected = NormalizedSnippet::from_code("let value = 42;\nvalue");
        let source = NormalizedSnippet::from_code(
            "fn answer() -> u8 {\nlet value = 42;\nvalue\n}\nfn other() {}",
        );

        assert!(expected.occurs_in(&source));
    }
}
