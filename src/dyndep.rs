use crate::manifest::expand;
use rapidhash::HashSetExt;
use rapidhash::fast::RapidHashSet as HashSet;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DyndepRecord {
    pub output: String,
    pub implicit_outputs: Vec<String>,
    pub implicit_inputs: Vec<String>,
    pub restat: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Colon,
    Pipe,
    Pipe2,
    Validation,
}

pub fn parse_dyndep(source: &str, path: &Path) -> Result<Vec<DyndepRecord>, String> {
    let missing_final_newline = !source.is_empty() && !source.ends_with('\n');
    let mut records: Vec<DyndepRecord> = Vec::new();
    let mut seen = HashSet::new();
    let mut version = false;
    let mut pending_record: Option<usize> = None;
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = strip_comment(raw).trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            let Some(record_id) = pending_record else {
                return Err(error(path, line_number, "unexpected indentation"));
            };
            let Some((name, value)) = line.trim().split_once('=') else {
                return Err(error(path, line_number, "expected 'restat = value'"));
            };
            if name.trim() != "restat" {
                return Err(error(path, line_number, "binding is not 'restat'"));
            }
            let value = value.trim();
            records[record_id].restat = !value.is_empty() && value != "0";
            pending_record = None;
            continue;
        }
        pending_record = None;
        if !version {
            let Some((name, value)) = line.split_once('=') else {
                return Err(error(
                    path,
                    line_number,
                    "expected 'ninja_dyndep_version = ...'",
                ));
            };
            let value = value.trim();
            let version_number = value.split_once('-').map_or(value, |(version, _)| version);
            if name.trim() != "ninja_dyndep_version" || !matches!(version_number, "1" | "1.0") {
                return Err(error(path, line_number, "unsupported dyndep version"));
            }
            version = true;
            continue;
        }
        let Some(build) = line.trim_start().strip_prefix("build ") else {
            return Err(error(path, line_number, "expected build statement"));
        };
        let tokens = lex(build);
        let colon = tokens
            .iter()
            .position(|token| *token == Token::Colon)
            .ok_or_else(|| error(path, line_number, "expected ':'"))?;
        let Some(Token::Word(output)) = tokens.first() else {
            return Err(error(path, line_number, "expected output path"));
        };
        let output = unescape(output);
        if !seen.insert(output.clone()) {
            return Err(error(
                path,
                line_number,
                &format!("multiple statements for '{output}'"),
            ));
        }
        let mut implicit_outputs = Vec::new();
        let mut output_pipe = false;
        for token in &tokens[1..colon] {
            match token {
                Token::Pipe if !output_pipe => output_pipe = true,
                Token::Word(word) if output_pipe => implicit_outputs.push(unescape(word)),
                Token::Word(_) => {
                    return Err(error(path, line_number, "explicit outputs not supported"));
                }
                _ => return Err(error(path, line_number, "invalid output list")),
            }
        }
        if tokens.get(colon + 1) != Some(&Token::Word("dyndep".to_owned())) {
            return Err(error(
                path,
                line_number,
                "expected build command name 'dyndep'",
            ));
        }
        let mut implicit_inputs = Vec::new();
        let mut input_pipe = false;
        for token in &tokens[colon + 2..] {
            match token {
                Token::Pipe if !input_pipe => input_pipe = true,
                Token::Word(word) if input_pipe => implicit_inputs.push(unescape(word)),
                Token::Word(_) => {
                    return Err(error(path, line_number, "explicit inputs not supported"));
                }
                Token::Pipe2 => {
                    return Err(error(path, line_number, "order-only inputs not supported"));
                }
                Token::Validation => {
                    return Err(error(path, line_number, "validation inputs not supported"));
                }
                _ => return Err(error(path, line_number, "invalid input list")),
            }
        }
        records.push(DyndepRecord {
            output,
            implicit_outputs,
            implicit_inputs,
            restat: false,
        });
        pending_record = Some(records.len() - 1);
    }
    if !version {
        return Err(error(path, 1, "expected 'ninja_dyndep_version = ...'"));
    }
    if missing_final_newline {
        return Err(error(
            path,
            source.bytes().filter(|byte| *byte == b'\n').count() + 1,
            "unexpected EOF",
        ));
    }
    Ok(records)
}

fn lex(input: &str) -> Vec<Token> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut chars = input.chars().peekable();
    let flush = |word: &mut String, result: &mut Vec<Token>| {
        if !word.is_empty() {
            result.push(Token::Word(std::mem::take(word)));
        }
    };
    while let Some(character) = chars.next() {
        match character {
            '$' => {
                word.push('$');
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            c if c.is_whitespace() => flush(&mut word, &mut result),
            ':' => {
                flush(&mut word, &mut result);
                result.push(Token::Colon);
            }
            '|' => {
                flush(&mut word, &mut result);
                if chars.peek() == Some(&'|') {
                    chars.next();
                    result.push(Token::Pipe2);
                } else if chars.peek() == Some(&'@') {
                    chars.next();
                    result.push(Token::Validation);
                } else {
                    result.push(Token::Pipe);
                }
            }
            c => word.push(c),
        }
    }
    flush(&mut word, &mut result);
    result
}

fn unescape(input: &str) -> String {
    expand(input, |_| None)
}

fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' && index + 1 < bytes.len() {
            index += 2;
        } else if bytes[index] == b'#' {
            return &line[..index];
        } else {
            index += 1;
        }
    }
    line
}

fn error(path: &Path, line: usize, message: &str) -> String {
    format!("{}:{line}: {message}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dynamic_inputs_outputs_and_restat() {
        let records = parse_dyndep(
            concat!(
                "ninja_dyndep_version = 1\n",
                "build out | module.mod: dyndep | generated.h\n",
                "  restat = 1\n",
            ),
            Path::new("deps.dd"),
        )
        .unwrap();
        assert_eq!(
            records,
            [DyndepRecord {
                output: "out".to_owned(),
                implicit_outputs: vec!["module.mod".to_owned()],
                implicit_inputs: vec!["generated.h".to_owned()],
                restat: true,
            }]
        );
    }

    #[test]
    fn treats_zero_restat_as_false() {
        let records = parse_dyndep(
            "ninja_dyndep_version = 1\nbuild out: dyndep\n  restat = 0\n",
            Path::new("deps.dd"),
        )
        .unwrap();
        assert!(!records[0].restat);
    }

    #[test]
    fn accepts_version_suffixes_and_requires_a_final_newline() {
        for version in ["1", "1.0", "1-extra", "1.0-extra"] {
            parse_dyndep(
                &format!("ninja_dyndep_version = {version}\n"),
                Path::new("deps.dd"),
            )
            .unwrap();
        }
        let error = parse_dyndep("ninja_dyndep_version = 1", Path::new("deps.dd")).unwrap_err();
        assert!(error.contains("unexpected EOF"));
    }

    #[test]
    fn rejects_validation_inputs() {
        let error = parse_dyndep(
            "ninja_dyndep_version = 1\nbuild out: dyndep |@ validation\n",
            Path::new("deps.dd"),
        )
        .unwrap_err();
        assert!(error.contains("validation inputs not supported"));
    }
}
