use crate::manifest::expand;
use crate::program_name;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DyndepRecord {
    pub output: String,
    pub implicit_outputs: Vec<String>,
    pub implicit_inputs: Vec<String>,
    pub restat: bool,
    pub(crate) origin: DyndepLocation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DyndepLocation {
    path: Arc<PathBuf>,
    line: usize,
    column: usize,
    source_line: String,
}

impl DyndepLocation {
    pub(crate) fn error(&self, message: impl Into<String>) -> String {
        DyndepDiagnostic {
            location: self.clone(),
            message: message.into(),
        }
        .render()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DyndepDiagnostic {
    location: DyndepLocation,
    message: String,
}

impl DyndepDiagnostic {
    fn at(
        source: &str,
        path: &Arc<PathBuf>,
        line_starts: &[usize],
        offset: usize,
        message: impl Into<String>,
    ) -> Self {
        let offset = offset.min(source.len());
        let line_index = line_starts
            .partition_point(|line_start| *line_start <= offset)
            .saturating_sub(1);
        let line_start = line_starts[line_index];
        let line_end = line_starts
            .get(line_index + 1)
            .map_or(source.len(), |next| next - 1);
        Self {
            location: DyndepLocation {
                path: Arc::clone(path),
                line: line_index + 1,
                column: offset.saturating_sub(line_start),
                source_line: source[line_start..line_end].to_owned(),
            },
            message: message.into(),
        }
    }

    fn ninja_message(&self) -> String {
        let location = &self.location;
        let mut rendered = format!(
            "{}:{}: {}\n",
            location.path.display(),
            location.line,
            self.message
        );
        if location.column == 0 || location.column >= 72 {
            return rendered;
        }

        let bytes = location.source_line.as_bytes();
        let truncated = bytes.len() > 72;
        let shown = &bytes[..bytes.len().min(72)];
        rendered.push_str(&String::from_utf8_lossy(shown));
        if truncated {
            rendered.push_str("...");
        }
        if cfg!(windows) && shown.ends_with(b"\r") {
            // Ninja's CRT adds another CR when it prints a retained CRLF line.
            rendered.push('\r');
        }
        rendered.push('\n');
        rendered.extend(std::iter::repeat_n(' ', location.column));
        rendered.push_str("^ near here");
        rendered
    }

    fn render(&self) -> String {
        if program_name() == "ninja" {
            self.ninja_message()
        } else {
            self.to_string()
        }
    }
}

impl fmt::Display for DyndepDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let location = &self.location;
        writeln!(
            formatter,
            "{}:{}:{}: error: {}",
            location.path.display(),
            location.line,
            location.column + 1,
            self.message
        )?;
        if !location.source_line.is_empty() {
            writeln!(
                formatter,
                "  {}",
                location.source_line.trim_end_matches('\r')
            )?;
            write!(formatter, "  {:>width$}^", "", width = location.column)?;
        }
        Ok(())
    }
}

pub fn parse_dyndep(source: &str, path: &Path) -> Result<Vec<DyndepRecord>, String> {
    Parser::new(source, path)
        .parse()
        .map_err(|diagnostic| diagnostic.render())
}

struct Parser<'a> {
    source: &'a str,
    path: Arc<PathBuf>,
    bytes: &'a [u8],
    line_starts: Vec<usize>,
    position: usize,
    have_version: bool,
    records: Vec<DyndepRecord>,
}

struct ParsedPath {
    value: String,
    delimiter: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, path: &'a Path) -> Self {
        let mut line_starts = Vec::with_capacity(source.len() / 40 + 1);
        line_starts.push(0);
        line_starts.extend(
            source
                .as_bytes()
                .iter()
                .enumerate()
                .filter_map(|(index, byte)| (*byte == b'\n').then_some(index + 1)),
        );
        Self {
            source,
            path: Arc::new(path.to_owned()),
            bytes: source.as_bytes(),
            line_starts,
            position: 0,
            have_version: false,
            records: Vec::new(),
        }
    }

    fn parse(mut self) -> Result<Vec<DyndepRecord>, DyndepDiagnostic> {
        loop {
            if self.position == self.bytes.len() {
                if self.have_version {
                    return Ok(self.records);
                }
                return Err(self.error(0, "expected 'ninja_dyndep_version = ...'"));
            }

            match self.bytes[self.position] {
                b'\n' => self.position += 1,
                b'\r' if self.bytes.get(self.position + 1) == Some(&b'\n') => {
                    self.position += 2;
                }
                b'#' => self.consume_comment(),
                b' ' => return Err(self.error(self.position, "unexpected indent")),
                b'\t' => {
                    return Err(self.error(self.position, "tabs are not allowed, use spaces"));
                }
                b'=' => return Err(self.error(self.position, "unexpected '='")),
                byte if is_ident(byte) => {
                    let (start, identifier) = self.read_ident().expect("identifier was detected");
                    if identifier == "build" {
                        if !self.have_version {
                            return Err(self.error(start, "expected 'ninja_dyndep_version = ...'"));
                        }
                        self.parse_edge()?;
                    } else if self.have_version {
                        return Err(self.error(start, "unexpected identifier"));
                    } else {
                        self.parse_version(identifier)?;
                        self.have_version = true;
                    }
                }
                _ => {
                    let offset = self.position;
                    let token = self.token_name(offset);
                    return Err(self.error(offset, format!("unexpected {token}")));
                }
            }
        }
    }

    fn parse_version(&mut self, name: &str) -> Result<(), DyndepDiagnostic> {
        self.expect_byte(b'=', "'='")?;
        let (value, end) = self.read_variable_value()?;
        if name != "ninja_dyndep_version" {
            return Err(self.error(end, "expected 'ninja_dyndep_version = ...'"));
        }
        let value = unescape(value);
        let (major, minor) = parse_version_components(&value);
        if major != 1 || minor != 0 {
            return Err(self.error(end, format!("unsupported 'ninja_dyndep_version = {value}'")));
        }
        Ok(())
    }

    fn parse_edge(&mut self) -> Result<(), DyndepDiagnostic> {
        let output = self.read_path()?;
        if output.value.is_empty() {
            return Err(self.error(output.delimiter, "expected path"));
        }
        let output_value = unescape(&output.value);
        if output_value.is_empty() {
            return Err(self.error(output.delimiter, "empty path"));
        }
        let origin = self.location(output.delimiter);

        let explicit_output = self.read_path()?;
        if !explicit_output.value.is_empty() {
            return Err(self.error(explicit_output.delimiter, "explicit outputs not supported"));
        }

        let mut implicit_outputs = Vec::new();
        if self.consume_single_pipe() {
            loop {
                let path = self.read_path()?;
                if path.value.is_empty() {
                    break;
                }
                implicit_outputs.push(unescape(&path.value));
            }
        }

        self.expect_byte(b':', "':'")?;
        let Some((rule_start, rule)) = self.read_ident() else {
            return Err(self.error(self.position, "expected build command name 'dyndep'"));
        };
        if rule != "dyndep" {
            return Err(self.error(rule_start, "expected build command name 'dyndep'"));
        }

        let explicit_input = self.read_path()?;
        if !explicit_input.value.is_empty() {
            return Err(self.error(explicit_input.delimiter, "explicit inputs not supported"));
        }

        let mut implicit_inputs = Vec::new();
        if self.consume_single_pipe() {
            loop {
                let path = self.read_path()?;
                if path.value.is_empty() {
                    break;
                }
                implicit_inputs.push(unescape(&path.value));
            }
        }
        if self.bytes.get(self.position..self.position + 2) == Some(b"||") {
            return Err(self.error(self.position, "order-only inputs not supported"));
        }
        self.expect_newline()?;

        let record_id = self.records.len();
        self.records.push(DyndepRecord {
            output: output_value,
            implicit_outputs,
            implicit_inputs,
            restat: false,
            origin,
        });

        if self.bytes.get(self.position) == Some(&b' ') {
            self.eat_spaces();
            let Some((_, key)) = self.read_ident() else {
                return Err(self.error(self.position, "expected variable name"));
            };
            self.expect_byte(b'=', "'='")?;
            let (value, end) = self.read_variable_value()?;
            if key != "restat" {
                return Err(self.error(end, "binding is not 'restat'"));
            }
            self.records[record_id].restat = !unescape(value).is_empty();
        }
        Ok(())
    }

    fn read_path(&mut self) -> Result<ParsedPath, DyndepDiagnostic> {
        let mut raw = String::new();
        loop {
            if self.position == self.bytes.len() {
                return Err(self.error(self.position, "unexpected EOF"));
            }
            let start = self.position;
            match self.bytes[self.position] {
                b' ' => {
                    self.position += 1;
                    self.eat_spaces();
                    return Ok(ParsedPath {
                        value: raw,
                        delimiter: start,
                    });
                }
                b'\t' | b'\n' | b':' | b'|' | b'#' => {
                    return Ok(ParsedPath {
                        value: raw,
                        delimiter: start,
                    });
                }
                b'\r' if self.bytes.get(self.position + 1) == Some(&b'\n') => {
                    return Ok(ParsedPath {
                        value: raw,
                        delimiter: start,
                    });
                }
                b'\r' => return Err(self.error(start, "lexing error")),
                b'$' => self.read_path_escape(&mut raw)?,
                _ => {
                    while self.bytes.get(self.position).is_some_and(|byte| {
                        !matches!(
                            byte,
                            b' ' | b'\t' | b'\n' | b'\r' | b':' | b'|' | b'#' | b'$'
                        )
                    }) {
                        self.position += 1;
                    }
                    raw.push_str(&self.source[start..self.position]);
                }
            }
        }
    }

    fn read_path_escape(&mut self, raw: &mut String) -> Result<(), DyndepDiagnostic> {
        let start = self.position;
        self.position += 1;
        let Some(&next) = self.bytes.get(self.position) else {
            return Err(self.error(start, "bad $-escape (literal $ must be written as $$)"));
        };
        match next {
            b'\n' => {
                self.position += 1;
                self.eat_spaces();
            }
            b'\r' if self.bytes.get(self.position + 1) == Some(&b'\n') => {
                self.position += 2;
                self.eat_spaces();
            }
            b'$' | b' ' | b':' | b'^' => {
                raw.push('$');
                raw.push(next as char);
                self.position += 1;
            }
            b'{' => {
                let name_start = self.position + 1;
                let mut end = name_start;
                while self.bytes.get(end).is_some_and(|byte| is_ident(*byte)) {
                    end += 1;
                }
                if end == name_start || self.bytes.get(end) != Some(&b'}') {
                    return Err(self.error(start, "bad $-escape (literal $ must be written as $$)"));
                }
                raw.push_str(&self.source[start..=end]);
                self.position = end + 1;
            }
            byte if is_ident(byte) => {
                let mut end = self.position + 1;
                while self.bytes.get(end).is_some_and(|byte| is_ident(*byte)) {
                    end += 1;
                }
                raw.push_str(&self.source[start..end]);
                self.position = end;
            }
            _ => {
                return Err(self.error(start, "bad $-escape (literal $ must be written as $$)"));
            }
        }
        Ok(())
    }

    fn read_variable_value(&mut self) -> Result<(&'a str, usize), DyndepDiagnostic> {
        let start = self.position;
        while self.position < self.bytes.len() {
            match self.bytes[self.position] {
                b'\n' => {
                    let end = self.position;
                    self.position += 1;
                    return Ok((&self.source[start..end], end));
                }
                b'\r' if self.bytes.get(self.position + 1) == Some(&b'\n') => {
                    let end = self.position;
                    self.position += 2;
                    return Ok((&self.source[start..end], end));
                }
                b'\r' => return Err(self.error(self.position, "lexing error")),
                _ => self.position += 1,
            }
        }
        Err(self.error(self.position, "unexpected EOF"))
    }

    fn read_ident(&mut self) -> Option<(usize, &'a str)> {
        let start = self.position;
        while self
            .bytes
            .get(self.position)
            .is_some_and(|byte| is_ident(*byte))
        {
            self.position += 1;
        }
        if self.position == start {
            return None;
        }
        let identifier = &self.source[start..self.position];
        self.eat_spaces();
        Some((start, identifier))
    }

    fn expect_byte(&mut self, expected: u8, name: &str) -> Result<(), DyndepDiagnostic> {
        if self.bytes.get(self.position) == Some(&expected) {
            self.position += 1;
            self.eat_spaces();
            return Ok(());
        }
        let token = self.token_name(self.position);
        let hint = if expected == b':' {
            " ($ also escapes ':')"
        } else {
            ""
        };
        Err(self.error(self.position, format!("expected {name}, got {token}{hint}")))
    }

    fn expect_newline(&mut self) -> Result<(), DyndepDiagnostic> {
        if self.bytes.get(self.position) == Some(&b'#') {
            self.consume_comment();
            return Ok(());
        }
        match self.bytes.get(self.position) {
            Some(b'\n') => {
                self.position += 1;
                Ok(())
            }
            Some(b'\r') if self.bytes.get(self.position + 1) == Some(&b'\n') => {
                self.position += 2;
                Ok(())
            }
            _ => {
                let token = self.token_name(self.position);
                Err(self.error(self.position, format!("expected newline, got {token}")))
            }
        }
    }

    fn consume_single_pipe(&mut self) -> bool {
        if self.bytes.get(self.position) != Some(&b'|')
            || matches!(self.bytes.get(self.position + 1), Some(b'|' | b'@'))
        {
            return false;
        }
        self.position += 1;
        self.eat_spaces();
        true
    }

    fn consume_comment(&mut self) {
        while self.position < self.bytes.len() && self.bytes[self.position] != b'\n' {
            self.position += 1;
        }
        if self.position < self.bytes.len() {
            self.position += 1;
        }
    }

    fn eat_spaces(&mut self) {
        loop {
            while self.bytes.get(self.position) == Some(&b' ') {
                self.position += 1;
            }
            if self.bytes.get(self.position..self.position + 2) == Some(b"$\n") {
                self.position += 2;
                continue;
            }
            if self.bytes.get(self.position..self.position + 3) == Some(b"$\r\n") {
                self.position += 3;
                continue;
            }
            break;
        }
    }

    fn token_name(&self, offset: usize) -> &'static str {
        match self.bytes.get(offset) {
            None => "eof",
            Some(b'\n' | b'\r') => "newline",
            Some(b'=') => "'='",
            Some(b':') => "':'",
            Some(b'|') if self.bytes.get(offset + 1) == Some(&b'|') => "'||'",
            Some(b'|') if self.bytes.get(offset + 1) == Some(&b'@') => "'|@'",
            Some(b'|') => "'|'",
            Some(b' ') => "indent",
            Some(byte) if is_ident(*byte) => "identifier",
            _ => "lexing error",
        }
    }

    fn location(&self, offset: usize) -> DyndepLocation {
        DyndepDiagnostic::at(self.source, &self.path, &self.line_starts, offset, "").location
    }

    fn error(&self, offset: usize, message: impl Into<String>) -> DyndepDiagnostic {
        DyndepDiagnostic::at(self.source, &self.path, &self.line_starts, offset, message)
    }
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

fn parse_version_components(version: &str) -> (i32, i32) {
    let (major, minor) = version.split_once('.').unwrap_or((version, ""));
    (atoi_prefix(major), atoi_prefix(minor))
}

fn atoi_prefix(value: &str) -> i32 {
    let bytes = value.as_bytes();
    let mut position = 0;
    while bytes.get(position).is_some_and(u8::is_ascii_whitespace) {
        position += 1;
    }
    let negative = bytes.get(position) == Some(&b'-');
    if negative || bytes.get(position) == Some(&b'+') {
        position += 1;
    }
    let start = position;
    let mut result = 0i32;
    while let Some(digit) = bytes.get(position).and_then(|byte| byte.checked_sub(b'0')) {
        if digit > 9 {
            break;
        }
        result = result.saturating_mul(10).saturating_add(i32::from(digit));
        position += 1;
    }
    if position == start {
        0
    } else if negative {
        -result
    } else {
        result
    }
}

fn unescape(input: &str) -> String {
    expand(input, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Result<Vec<DyndepRecord>, DyndepDiagnostic> {
        Parser::new(source, Path::new("input")).parse()
    }

    #[test]
    fn parses_dynamic_inputs_outputs_and_restat() {
        let records = parse(concat!(
            "ninja_dyndep_version = 1\n",
            "build out | module.mod: dyndep | generated.h\n",
            "  restat = 1\n",
        ))
        .unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.output, "out");
        assert_eq!(record.implicit_outputs, ["module.mod"]);
        assert_eq!(record.implicit_inputs, ["generated.h"]);
        assert!(record.restat);
    }

    #[test]
    fn nonempty_restat_values_are_true_like_ninja() {
        for (value, expected) in [("", false), ("0", true), ("1", true)] {
            let records = parse(&format!(
                "ninja_dyndep_version = 1\nbuild out: dyndep\n  restat = {value}\n"
            ))
            .unwrap();
            assert_eq!(records[0].restat, expected, "value={value:?}");
        }
    }

    #[test]
    fn accepts_upstream_version_and_layout_corpus() {
        for source in [
            "ninja_dyndep_version = 1\n",
            "ninja_dyndep_version = 1-extra\n",
            "ninja_dyndep_version = 1.0\n",
            "ninja_dyndep_version = 1.0-extra\n",
            "# comment\nninja_dyndep_version = 1\n",
            "\nninja_dyndep_version = 1\n",
            "ninja_dyndep_version = 1\r\n",
            "# comment\r\nninja_dyndep_version = 1\r\n",
            "\r\nninja_dyndep_version = 1\r\n",
            "ninja_dyndep_version = 1\nbuild out: dyndep\n",
            "ninja_dyndep_version = 1\nbuild out | : dyndep |\n",
            "ninja_dyndep_version = 1\nbuild out: dyndep | impin\n",
            "ninja_dyndep_version = 1\nbuild out: dyndep | impin1 impin2\n",
            "ninja_dyndep_version = 1\nbuild out | impout: dyndep\n",
            "ninja_dyndep_version = 1\nbuild out | impout1 impout2: dyndep\n",
            "ninja_dyndep_version = 1\nbuild out | impout: dyndep | impin\n",
            "ninja_dyndep_version = 1\nbuild out: dyndep\n  restat = 1\n",
            "ninja_dyndep_version = 1\nbuild otherout: dyndep\n",
            concat!(
                "ninja_dyndep_version = 1\n",
                "build out: dyndep\n",
                "build out2: dyndep\n",
                "  restat = 1\n",
            ),
        ] {
            parse(source).unwrap_or_else(|error| panic!("{source:?}: {error}"));
        }
    }

    #[test]
    fn rejection_diagnostics_match_upstream_parser() {
        let cases = [
            ("", "input:1: expected 'ninja_dyndep_version = ...'\n"),
            (
                "ninja_dyndep_version = 1.0",
                "input:1: unexpected EOF\nninja_dyndep_version = 1.0\n                          ^ near here",
            ),
            (
                "ninja_dyndep_version = 0\n",
                "input:1: unsupported 'ninja_dyndep_version = 0'\nninja_dyndep_version = 0\n                        ^ near here",
            ),
            (
                "ninja_dyndep_version = 1.1\n",
                "input:1: unsupported 'ninja_dyndep_version = 1.1'\nninja_dyndep_version = 1.1\n                          ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nninja_dyndep_version = 1\n",
                "input:2: unexpected identifier\n",
            ),
            (
                "not_ninja_dyndep_version = 1\n",
                "input:1: expected 'ninja_dyndep_version = ...'\nnot_ninja_dyndep_version = 1\n                            ^ near here",
            ),
            (
                "build out: dyndep\n",
                "input:1: expected 'ninja_dyndep_version = ...'\n",
            ),
            ("= 1\n", "input:1: unexpected '='\n"),
            (" = 1\n", "input:1: unexpected indent\n"),
            (
                "ninja_dyndep_version = 1\nbuild",
                "input:2: unexpected EOF\nbuild\n     ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild :\n",
                "input:2: expected path\nbuild :\n      ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild out",
                "input:2: unexpected EOF\nbuild out\n         ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild out:",
                "input:2: expected build command name 'dyndep'\nbuild out:\n          ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild out: touch",
                "input:2: expected build command name 'dyndep'\nbuild out: touch\n           ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild out: dyndep",
                "input:2: unexpected EOF\nbuild out: dyndep\n                 ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild out exp: dyndep\n",
                "input:2: explicit outputs not supported\nbuild out exp: dyndep\n             ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild out: dyndep exp\n",
                "input:2: explicit inputs not supported\nbuild out: dyndep exp\n                     ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild out: dyndep ||\n",
                "input:2: order-only inputs not supported\nbuild out: dyndep ||\n                  ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild out: dyndep\n  not_restat = 1\n",
                "input:3: binding is not 'restat'\n  not_restat = 1\n                ^ near here",
            ),
            (
                "ninja_dyndep_version = 1\nbuild out: dyndep\n  restat = 1\n  restat = 1\n",
                "input:4: unexpected indent\n",
            ),
        ];
        for (source, expected) in cases {
            let actual = parse(source).unwrap_err().ninja_message();
            assert_eq!(actual, expected, "source={source:?}");
        }
    }
}
