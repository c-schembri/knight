use rapidhash::HashSetExt;
use rapidhash::fast::RapidHashSet as HashSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Depfile {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
}

pub fn parse_depfile(source: &str) -> Result<Depfile, String> {
    let bytes = source.as_bytes();
    let mut result = Depfile::default();
    let mut output_set = HashSet::new();
    let mut input_set = HashSet::new();
    let mut token = Vec::new();
    let mut parsing_outputs = true;
    let mut poisoned_input = false;
    let mut have_separator = false;
    let mut saw_content = false;
    let mut index = 0;

    while index <= bytes.len() {
        let byte = bytes.get(index).copied();
        match byte {
            None => {
                flush_token(
                    &mut token,
                    parsing_outputs,
                    &mut poisoned_input,
                    &mut result,
                    &mut output_set,
                    &mut input_set,
                )?;
                break;
            }
            Some(b'\r') if bytes.get(index + 1) == Some(&b'\n') => {
                flush_token(
                    &mut token,
                    parsing_outputs,
                    &mut poisoned_input,
                    &mut result,
                    &mut output_set,
                    &mut input_set,
                )?;
                parsing_outputs = true;
                poisoned_input = false;
                index += 2;
            }
            Some(b'\n') => {
                flush_token(
                    &mut token,
                    parsing_outputs,
                    &mut poisoned_input,
                    &mut result,
                    &mut output_set,
                    &mut input_set,
                )?;
                parsing_outputs = true;
                poisoned_input = false;
                index += 1;
            }
            Some(byte) if byte.is_ascii_whitespace() => {
                flush_token(
                    &mut token,
                    parsing_outputs,
                    &mut poisoned_input,
                    &mut result,
                    &mut output_set,
                    &mut input_set,
                )?;
                index += 1;
            }
            Some(b':')
                if parsing_outputs
                    && bytes
                        .get(index + 1)
                        .is_none_or(|next| next.is_ascii_whitespace()) =>
            {
                flush_token(
                    &mut token,
                    true,
                    &mut poisoned_input,
                    &mut result,
                    &mut output_set,
                    &mut input_set,
                )?;
                parsing_outputs = false;
                have_separator = true;
                saw_content = true;
                index += 1;
            }
            Some(b'\\') => {
                let start = index;
                while bytes.get(index) == Some(&b'\\') {
                    index += 1;
                }
                let count = index - start;
                match bytes.get(index).copied() {
                    Some(b' ') if count % 2 == 1 => {
                        token.extend(std::iter::repeat_n(b'\\', count / 2));
                        token.push(b' ');
                        saw_content = true;
                        index += 1;
                    }
                    Some(b' ') => {
                        token.extend(std::iter::repeat_n(b'\\', count));
                        flush_token(
                            &mut token,
                            parsing_outputs,
                            &mut poisoned_input,
                            &mut result,
                            &mut output_set,
                            &mut input_set,
                        )?;
                        index += 1;
                    }
                    Some(b'#') => {
                        token.extend(std::iter::repeat_n(b'\\', count.saturating_sub(1)));
                        token.push(b'#');
                        saw_content = true;
                        index += 1;
                    }
                    Some(b':') => {
                        let separator = bytes
                            .get(index + 1)
                            .is_none_or(|next| next.is_ascii_whitespace());
                        if separator {
                            token.extend(std::iter::repeat_n(b'\\', count));
                            flush_token(
                                &mut token,
                                parsing_outputs,
                                &mut poisoned_input,
                                &mut result,
                                &mut output_set,
                                &mut input_set,
                            )?;
                            parsing_outputs = false;
                            have_separator = true;
                        } else {
                            token.extend(std::iter::repeat_n(b'\\', count.saturating_sub(1)));
                            token.push(b':');
                        }
                        saw_content = true;
                        index += 1;
                    }
                    Some(b'\r') if bytes.get(index + 1) == Some(&b'\n') => {
                        flush_token(
                            &mut token,
                            parsing_outputs,
                            &mut poisoned_input,
                            &mut result,
                            &mut output_set,
                            &mut input_set,
                        )?;
                        index += 2;
                    }
                    Some(b'\n') => {
                        flush_token(
                            &mut token,
                            parsing_outputs,
                            &mut poisoned_input,
                            &mut result,
                            &mut output_set,
                            &mut input_set,
                        )?;
                        index += 1;
                    }
                    _ => {
                        token.extend(std::iter::repeat_n(b'\\', count));
                        saw_content = true;
                    }
                }
            }
            Some(b'$') if bytes.get(index + 1) == Some(&b'$') => {
                token.push(b'$');
                saw_content = true;
                index += 2;
            }
            Some(byte) => {
                token.push(byte);
                saw_content = true;
                index += 1;
            }
        }
    }

    if saw_content && !have_separator {
        Err("expected ':' in depfile".to_owned())
    } else {
        Ok(result)
    }
}

fn flush_token(
    token: &mut Vec<u8>,
    parsing_outputs: bool,
    poisoned_input: &mut bool,
    result: &mut Depfile,
    output_set: &mut HashSet<String>,
    input_set: &mut HashSet<String>,
) -> Result<(), String> {
    if token.is_empty() {
        return Ok(());
    }
    let value = String::from_utf8(std::mem::take(token))
        .map_err(|_| "depfile path is not valid UTF-8".to_owned())?;
    if parsing_outputs {
        if input_set.contains(&value) {
            *poisoned_input = true;
        } else if output_set.insert(value.clone()) {
            result.outputs.push(value);
        }
    } else {
        if *poisoned_input {
            return Err("inputs may not also have inputs".to_owned());
        }
        if input_set.insert(value.clone()) {
            result.inputs.push(value);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_rules_outputs_and_continuations() {
        let parsed = parse_depfile("foo bar: x y\\\n z\nfoo: z q\n").unwrap();
        assert_eq!(parsed.outputs, ["foo", "bar"]);
        assert_eq!(parsed.inputs, ["x", "y", "z", "q"]);
    }

    #[test]
    fn follows_ninja_backslash_and_dollar_escaping() {
        let source = format!(
            r"a\ bc\#d$$.o: {}  {} \\share\info\\#1",
            "\\".repeat(5),
            "\\".repeat(4)
        );
        let parsed = parse_depfile(&source).unwrap();
        assert_eq!(parsed.outputs, ["a bc#d$.o"]);
        assert_eq!(parsed.inputs, ["\\\\ ", "\\\\\\\\", "\\\\share\\info\\#1"]);
    }

    #[test]
    fn rejects_missing_separator_and_poisoned_inputs() {
        assert_eq!(
            parse_depfile("foo.o foo.c").unwrap_err(),
            "expected ':' in depfile"
        );
        assert_eq!(
            parse_depfile("foo: x\nx: alsoin\n").unwrap_err(),
            "inputs may not also have inputs"
        );
    }

    #[test]
    fn preserves_windows_drive_colons() {
        let parsed = parse_depfile("foo.o: C:\\src\\answer.c //?/c:/header.h\n").unwrap();
        assert_eq!(parsed.inputs, ["C:\\src\\answer.c", "//?/c:/header.h"]);
    }

    #[test]
    fn matches_ninja_mp_and_mixed_rule_corpus() {
        for source in [
            "foo: x y z\nx:\ny:\nz:\n",
            "foo: x\nx:\nfoo: y\ny:\nfoo: z\nz:\n",
            "foo: x\\\n     y\nfoo \\\nfoo: z\n",
            " foo: x\r\n foo: y\r\n foo: z\r\n",
        ] {
            let parsed = parse_depfile(source).unwrap();
            assert_eq!(parsed.outputs, ["foo"], "source={source:?}");
            assert_eq!(parsed.inputs, ["x", "y", "z"], "source={source:?}");
        }
    }

    #[test]
    fn matches_ninja_special_character_and_escaped_colon_cases() {
        let parsed = parse_depfile(concat!(
            "C:/Program\\ Files\\ (x86)/Microsoft\\ crtdefs.h: \\\n",
            " en@quot.header~ t+t-x!=1 \\\n",
            " openldap/cn={0}core.ldif\\\n",
            " Fussball\\\n",
            " a[1]b@2%c",
        ))
        .unwrap();
        assert_eq!(
            parsed.outputs,
            ["C:/Program Files (x86)/Microsoft crtdefs.h"]
        );
        assert_eq!(
            parsed.inputs,
            [
                "en@quot.header~",
                "t+t-x!=1",
                "openldap/cn={0}core.ldif",
                "Fussball",
                "a[1]b@2%c",
            ]
        );

        let parsed = parse_depfile("foo1\\: x\nfoo1\\:\nfoo1\\:\r\nfoo1\\:\t\nfoo1\\:").unwrap();
        assert_eq!(parsed.outputs, ["foo1\\"]);
        assert_eq!(parsed.inputs, ["x"]);
    }
}
