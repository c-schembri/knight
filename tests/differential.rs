use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

fn run(executable: &Path, directory: &Path, arguments: &[&str]) -> Output {
    Command::new(executable)
        .current_dir(directory)
        .args(arguments)
        .output()
        .unwrap()
}

fn install_ninja_alias(knight: &Path, alias: &Path) {
    #[cfg(windows)]
    fs::copy(knight, alias).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(knight, alias).unwrap();
}

#[test]
fn ninja_and_knight_exchange_build_logs() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let temp = tempdir().unwrap();
    let command = if cfg!(windows) {
        "cmd /c echo hello>$out"
    } else {
        "printf hello > $out"
    };
    fs::write(
        temp.path().join("build.ninja"),
        format!("rule write\n  command = {command}\nbuild out.txt: write\ndefault out.txt\n"),
    )
    .unwrap();
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    let first = run(ninja, temp.path(), &[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let knight_noop = run(knight, temp.path(), &[]);
    assert!(
        knight_noop.status.success(),
        "{}",
        String::from_utf8_lossy(&knight_noop.stderr)
    );
    assert!(String::from_utf8_lossy(&knight_noop.stdout).contains("no work"));

    fs::remove_file(temp.path().join("out.txt")).unwrap();
    let second = run(knight, temp.path(), &[]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let ninja_noop = run(ninja, temp.path(), &[]);
    assert!(
        ninja_noop.status.success(),
        "{}",
        String::from_utf8_lossy(&ninja_noop.stderr)
    );
    assert!(String::from_utf8_lossy(&ninja_noop.stdout).contains("no work"));
}

#[test]
fn upstream_disk_interface_bad_stat_path_matches_ninja_alias() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    #[cfg(windows)]
    let input = r"cc$:\foo";
    #[cfg(not(windows))]
    let input = "x".repeat(512);
    fs::write(
        temp.path().join("build.ninja"),
        format!("rule echo\n  command = echo unused\nbuild out: echo {input}\ndefault out\n"),
    )
    .unwrap();
    let alias = temp
        .path()
        .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
    install_ninja_alias(knight, &alias);

    let expected = run(Path::new(&ninja), temp.path(), &["-n"]);
    let actual = run(&alias, temp.path(), &["-n"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn upstream_disk_interface_remove_file_and_directory_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let mut expected = None;
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule generate\n  command = echo unused\n",
                "build file-to-remove directory-to-remove: generate\n",
            ),
        )
        .unwrap();
        fs::write(temp.path().join("file-to-remove"), []).unwrap();
        fs::create_dir(temp.path().join("directory-to-remove")).unwrap();
        #[cfg(windows)]
        {
            let mut permissions = fs::metadata(temp.path().join("file-to-remove"))
                .unwrap()
                .permissions();
            permissions.set_readonly(true);
            fs::set_permissions(temp.path().join("file-to-remove"), permissions).unwrap();
        }

        let first = run(executable, temp.path(), &["-t", "clean"]);
        let second = run(executable, temp.path(), &["-t", "clean"]);
        let result = (
            first.status.code(),
            first.stdout,
            first.stderr,
            second.status.code(),
            second.stdout,
            second.stderr,
        );
        if let Some(expected) = &expected {
            assert_eq!(&result, expected);
        } else {
            expected = Some(result);
        }
        assert!(!temp.path().join("file-to-remove").exists());
        assert!(!temp.path().join("directory-to-remove").exists());
    }
}

#[cfg(windows)]
#[test]
fn upstream_disk_interface_long_path_stat_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let mut source = std::path::PathBuf::from("source");
    for index in 0..5 {
        source.push(format!("component-{index}-{}", "x".repeat(48)));
    }
    source.push("input");
    fs::create_dir_all(temp.path().join(source.parent().unwrap())).unwrap();
    fs::write(temp.path().join(&source), []).unwrap();
    let source = source.to_string_lossy().replace('\\', "/");
    assert!(temp.path().join(&source).as_os_str().len() > 260);
    fs::write(
        temp.path().join("build.ninja"),
        format!("rule echo\n  command = echo unused\nbuild out: echo {source}\ndefault out\n"),
    )
    .unwrap();

    let expected = run(Path::new(&ninja), temp.path(), &["-n"]);
    let actual = run(knight, temp.path(), &["-n"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[cfg(windows)]
#[test]
fn canonical_manifest_path_still_regenerates_and_reloads() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let manifest = |value: &str| {
        format!(
            "rule regen\n  command = cmd /d /c copy /y configured.ninja build.ninja >nul\n  generator = 1\n\
             build build.ninja: regen configured.ninja\n\
             rule write\n  command = cmd /d /c echo {value}>out\n\
             build out: write\ndefault out\n"
        )
    };
    fs::write(temp.path().join("build.ninja"), manifest("old")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(temp.path().join("configured.ninja"), manifest("new")).unwrap();
    let built = run(knight, temp.path(), &["-f", "./build.ninja"]);
    assert!(
        built.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("out")).unwrap().trim(),
        "new"
    );
}

#[test]
fn manifest_regeneration_stops_at_ninjas_cycle_limit() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let command = if cfg!(windows) {
        "cmd /d /c copy /b /y template.ninja build.ninja >nul"
    } else {
        "cp template.ninja build.ninja"
    };
    let manifest = format!(
        "rule regen\n  command = {command}\n  generator = 1\nbuild build.ninja: regen force\nbuild force: phony\nbuild all: phony\ndefault all\n"
    );
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("template.ninja"), &manifest).unwrap();
        fs::write(temp.path().join("build.ninja"), &manifest).unwrap();
        let output = run(executable, temp.path(), &["--quiet"]);
        assert_eq!(output.status.code(), Some(1), "{}", executable.display());
        assert!(output.stdout.is_empty(), "{}", executable.display());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("manifest 'build.ninja' still dirty after 100 tries"),
            "{}: {}",
            executable.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn clean_restat_manifest_does_not_trigger_reload_cycles() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let (regen, build) = if cfg!(windows) {
        (
            "cmd /d /c echo regen>>regen.txt",
            "cmd /d /c echo built>$out",
        )
    } else {
        ("printf 'regen\\n' >> regen.txt", "printf built > $out")
    };
    let manifest = format!(
        "rule regen\n  command = {regen}\n  generator = 1\n  restat = 1\nrule make\n  command = {build}\nbuild build.ninja: regen force\nbuild force: phony\nbuild out: make\ndefault out\n"
    );
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), &manifest).unwrap();
        let output = run(executable, temp.path(), &[]);
        assert!(
            output.status.success(),
            "{}: stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(temp.path().join("out").exists(), "{}", executable.display());
        assert_eq!(
            fs::read_to_string(temp.path().join("regen.txt"))
                .unwrap()
                .lines()
                .count(),
            1,
            "{}",
            executable.display()
        );
    }
}

#[test]
fn dependency_log_validations_and_declared_dirty_short_circuit_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let (copy, compile) = if cfg!(windows) {
        (
            "cmd /d /c type $in > $out",
            "cmd /d /c \"type $in > $out && echo out2: out>out2.d\"",
        )
    } else {
        (
            "cat $in > $out",
            "cat $in > $out && printf 'out2: out\\n' > out2.d",
        )
    };
    let manifest = format!(
        "rule copy\n  command = {copy}\n\
         rule compile\n  command = {compile}\n\
         build out: copy in |@ validate\n\
         build validate: copy in2 | out\n\
         build out2: compile in3\n  deps = gcc\n  depfile = out2.d\n\
         default out2\n"
    );

    let mut observed = Vec::new();
    for executable in [ninja, knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), &manifest).unwrap();
        for (path, contents) in [("in", "in"), ("in2", "in2"), ("in3", "in3")] {
            fs::write(temp.path().join(path), contents).unwrap();
        }

        let first = run(executable, temp.path(), &["-j1"]);
        assert!(
            first.status.success(),
            "{} phase 1: stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("in2"), "changed").unwrap();
        fs::write(temp.path().join("in"), "changed").unwrap();
        let discovered_dirty = run(executable, temp.path(), &["-j1"]);
        assert!(
            discovered_dirty.status.success(),
            "{} phase 2: stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&discovered_dirty.stdout),
            String::from_utf8_lossy(&discovered_dirty.stderr)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("in2"), "changed again").unwrap();
        fs::write(temp.path().join("in3"), "changed").unwrap();
        let declared_dirty = run(executable, temp.path(), &["-j1"]);
        assert!(
            declared_dirty.status.success(),
            "{} phase 3: stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&declared_dirty.stdout),
            String::from_utf8_lossy(&declared_dirty.stderr)
        );
        observed.push([first.stdout, discovered_dirty.stdout, declared_dirty.stdout]);
    }

    assert_eq!(observed[1], observed[0]);
}

#[cfg(unix)]
#[test]
fn multi_output_restat_only_cleans_dependents_of_unchanged_outputs() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let manifest = concat!(
        "rule generate\n",
        "  command = if [ ! -e out1 ]; then printf first > out1; python3 -c \"import os; os.utime('out1', (1, 1))\"; printf keep > out2; else printf second > out1; python3 -c \"import os; os.utime('out1', (2, 2))\"; fi\n",
        "  restat = 1\n",
        "rule copy\n",
        "  command = cat $in > $out\n",
        "build out1 out2: generate source\n",
        "build final1: copy out1\n",
        "build final2: copy out2\n",
        "default final1 final2\n",
    );

    let mut observed = Vec::new();
    for executable in [ninja, knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("source"), "first").unwrap();
        let first = run(executable, temp.path(), &["-j1"]);
        assert!(
            first.status.success(),
            "{} phase 1: stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("source"), "second").unwrap();
        let second = run(executable, temp.path(), &["-j1"]);
        assert!(
            second.status.success(),
            "{} phase 2: stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&second.stdout),
            String::from_utf8_lossy(&second.stderr)
        );
        observed.push(second.stdout);
    }

    assert_eq!(observed[1], observed[0]);
}

#[cfg(windows)]
#[test]
fn failed_command_status_and_exit_code_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "rule fail\n  command = cmd /d /c exit 7\nbuild out: fail\ndefault out\n",
    )
    .unwrap();
    let expected = run(Path::new(&ninja), temp.path(), &[]);
    let actual = run(knight, temp.path(), &[]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert!(
        String::from_utf8_lossy(&actual.stdout).contains("FAILED: [code=7] out "),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&actual.stdout),
        String::from_utf8_lossy(&actual.stderr)
    );
}

#[cfg(windows)]
#[test]
fn failed_command_header_precedes_buffered_output_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule fail\n",
            "  command = cmd /d /c \"echo before-failure & exit 7\"\n",
            "build out: fail\ndefault out\n",
        ),
    )
    .unwrap();

    for executable in [Path::new(&ninja), knight] {
        let result = run(executable, temp.path(), &[]);
        assert_eq!(result.status.code(), Some(7));
        let stdout = String::from_utf8_lossy(&result.stdout);
        let failed = stdout.find("FAILED: [code=7] out").unwrap();
        let command = failed + stdout[failed..].find("cmd /d /c").unwrap();
        let output = command + stdout[command..].find("before-failure").unwrap();
        assert!(failed < command && command < output, "stdout={stdout:?}");
    }
}

#[cfg(windows)]
#[test]
fn dependency_extraction_failure_is_reported_as_a_buffered_subcommand_failure() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = concat!(
        "rule cc\n",
        "  command = cmd /d /c \"echo compiler-output & echo malformed > out.d & echo object > out\"\n",
        "  deps = gcc\n  depfile = out.d\n",
        "build out: cc\ndefault out\n",
    );

    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let result = run(executable, temp.path(), &[]);
        assert_eq!(result.status.code(), Some(1));
        let stdout = String::from_utf8_lossy(&result.stdout);
        let failed = stdout.find("FAILED: [code=1] out").unwrap();
        let compiler = failed + stdout[failed..].find("compiler-output").unwrap();
        let dep_error = compiler + stdout[compiler..].find("expected ':'").unwrap();
        assert!(
            failed < compiler && compiler < dep_error,
            "stdout={stdout:?}"
        );
    }
}

#[cfg(windows)]
#[test]
fn redirected_color_environment_precedence_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = concat!(
        "rule color\n",
        "  command = powershell -NoProfile -Command \"[Console]::Out.Write([char]27 + '[31mRED' + [char]27 + '[0m')\"\n",
        "build out: color\ndefault out\n",
    );

    for (name, environment, colored) in [
        ("default", &[][..], false),
        ("clicolor", &[("CLICOLOR_FORCE", "1")][..], true),
        (
            "no-color-over-clicolor",
            &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")][..],
            false,
        ),
        (
            "force-over-no-color",
            &[("NO_COLOR", "1"), ("FORCE_COLOR", "1")][..],
            true,
        ),
        (
            "zero-values",
            &[("NO_COLOR", "0"), ("FORCE_COLOR", "0")][..],
            false,
        ),
    ] {
        for executable in [Path::new(&ninja), knight] {
            let temp = tempdir().unwrap();
            fs::write(temp.path().join("build.ninja"), manifest).unwrap();
            let mut command = Command::new(executable);
            command.current_dir(temp.path()).env_remove("NO_COLOR");
            command
                .env_remove("CLICOLOR_FORCE")
                .env_remove("FORCE_COLOR");
            for (key, value) in environment {
                command.env(key, value);
            }
            let result = command.output().unwrap();
            assert!(result.status.success(), "case={name}");
            assert_eq!(
                result.stdout.contains(&0x1b),
                colored,
                "case={name} executable={} stdout={:?}",
                executable.display(),
                result.stdout
            );
        }
    }

    for (name, environment, colored) in [
        ("forced-failure", &[("FORCE_COLOR", "1")][..], true),
        ("plain-failure", &[("NO_COLOR", "1")][..], false),
    ] {
        for executable in [Path::new(&ninja), knight] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                "rule fail\n  command = cmd /d /c exit 1\nbuild out: fail\ndefault out\n",
            )
            .unwrap();
            let mut command = Command::new(executable);
            command.current_dir(temp.path()).env_remove("NO_COLOR");
            command
                .env_remove("CLICOLOR_FORCE")
                .env_remove("FORCE_COLOR");
            for (key, value) in environment {
                command.env(key, value);
            }
            let result = command.output().unwrap();
            assert!(!result.status.success(), "case={name}");
            assert_eq!(
                result.stdout.contains(&0x1b),
                colored,
                "case={name} executable={} stdout={:?}",
                executable.display(),
                result.stdout
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn command_output_preserves_stream_order_and_strips_ansi_when_piped() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule emit\n",
            "  command = powershell -NoProfile -Command \"[Console]::Out.WriteLine('one'); ",
            "[Console]::Error.WriteLine('two'); [Console]::Out.Write([char]27); ",
            "[Console]::Out.Write('[31mthree'); [Console]::Out.Write([char]27); ",
            "[Console]::Out.WriteLine('[0m'); Set-Content $out done\"\n",
            "  description = EMIT\nbuild out: emit\ndefault out\n",
        ),
    )
    .unwrap();
    let actual = run(knight, temp.path(), &[]);
    assert!(actual.status.success());
    let stdout = String::from_utf8_lossy(&actual.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines, ["[1/1] EMIT", "one", "two", "three"]);
    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        fs::remove_file(temp.path().join("out")).unwrap();
        let expected = run(Path::new(&ninja), temp.path(), &[]);
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
    }
}

#[cfg(windows)]
#[test]
fn response_file_paths_with_spaces_are_not_shell_escaped_on_disk() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule write\n",
            "  command = cmd /d /c type $rspfile > $out\n",
            "  rspfile = $out.rsp\n",
            "  rspfile_content = $in\n",
            "build output$ file: write source$ file\n",
            "default output$ file\n",
        ),
    )
    .unwrap();
    fs::write(temp.path().join("source file"), "source").unwrap();
    let built = run(knight, temp.path(), &["-d", "keeprsp"]);
    assert!(
        built.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(temp.path().join("output file").exists());
    assert!(temp.path().join("output file.rsp").exists());
}

#[cfg(windows)]
#[test]
fn response_file_newlines_use_windows_text_mode_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule rsp\n",
                "  command = cmd /d /c copy /b $rspfile $out\n",
                "  rspfile = $out.rsp\n",
                "  rspfile_content = $in_newline\n",
                "build out: rsp a b\n",
                "default out\n",
            ),
        )
        .unwrap();
        fs::write(temp.path().join("a"), "a").unwrap();
        fs::write(temp.path().join("b"), "b").unwrap();
        let built = run(executable, temp.path(), &[]);
        assert!(built.status.success(), "{}", executable.display());
        assert_eq!(fs::read(temp.path().join("out")).unwrap(), b"a\r\nb");
    }
}

#[cfg(windows)]
#[test]
fn ninja_and_knight_share_canonical_path_identity() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    for (producer, consumer) in [(knight, ninja), (ninja, knight)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule copy\n  command = cmd /d /c type $in > $out\n",
                "build ./out: copy sources/../source\n",
                "default nested/../out\n",
            ),
        )
        .unwrap();
        fs::write(temp.path().join("source"), "value\n").unwrap();
        let built = run(producer, temp.path(), &[]);
        assert!(
            built.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&built.stdout),
            String::from_utf8_lossy(&built.stderr)
        );
        let noop = run(consumer, temp.path(), &["./out"]);
        assert!(
            noop.status.success() && String::from_utf8_lossy(&noop.stdout).contains("no work"),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&noop.stdout),
            String::from_utf8_lossy(&noop.stderr)
        );
    }
}

#[cfg(windows)]
#[test]
fn utf8_and_long_manifest_paths_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let mut directory = std::path::PathBuf::from("build-构建");
    for index in 0..5 {
        directory.push(format!("component-{index}-{}", "x".repeat(48)));
    }
    fs::create_dir_all(temp.path().join(&directory)).unwrap();
    let manifest = directory.join("计划.ninja");
    fs::write(temp.path().join(&manifest), "build unicode-target: phony\n").unwrap();
    assert!(manifest.as_os_str().len() > 260);

    let manifest = manifest.to_string_lossy().into_owned();
    let arguments = ["-f", manifest.as_str(), "-t", "targets", "all"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert!(expected.status.success());
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[cfg(windows)]
#[test]
fn inputless_phony_target_forces_rebuilds() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let mut executables = vec![knight];
    let ninja;
    if let Some(path) = std::env::var_os("KNIGHT_NINJA") {
        ninja = path;
        executables.push(Path::new(&ninja));
    }
    for executable in executables {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule append\n  command = cmd /d /c echo run>>runs.txt && echo output>$out\n",
                "build force: phony\n",
                "build out: append force\n",
                "default out\n",
            ),
        )
        .unwrap();
        for _ in 0..2 {
            let built = run(executable, temp.path(), &[]);
            assert!(built.status.success());
        }
        assert_eq!(
            fs::read_to_string(temp.path().join("runs.txt"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
}

#[cfg(windows)]
#[test]
fn zero_depth_pool_is_unlimited_during_execution() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let mut executables = vec![knight];
    let ninja;
    if let Some(path) = std::env::var_os("KNIGHT_NINJA") {
        ninja = path;
        executables.push(Path::new(&ninja));
    }
    for executable in executables {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "pool unlimited\n  depth = 0\n",
                "rule write\n  command = cmd /d /c echo built>$out\n  pool = unlimited\n",
                "build out: write\ndefault out\n",
            ),
        )
        .unwrap();
        let built = run(executable, temp.path(), &[]);
        assert!(
            built.status.success() && temp.path().join("out").exists(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&built.stdout),
            String::from_utf8_lossy(&built.stderr)
        );
    }
}

#[test]
fn zero_depth_pool_does_not_throttle_ninja_dry_runs() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "pool unlimited\n  depth = 0\n",
            "rule run\n  command = echo $out\n  pool = unlimited\n",
            "build root: run\n",
            "build after: run root\n",
            "build later: run\n",
            "build all: phony after later\n",
            "default all\n",
        ),
    )
    .unwrap();

    let arguments = ["-n", "-j1"];
    let actual = run(knight, temp.path(), &arguments);
    assert!(actual.status.success());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["[1/3] echo root", "[2/3] echo later", "[3/3] echo after",]
    );
    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn bounded_pools_reserve_ready_work_like_ninja() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let command = if cfg!(windows) {
        "cmd /d /c \"echo $out>>order.txt && echo built>$out\""
    } else {
        "printf '%s\\n' $out >> order.txt && touch $out"
    };
    let manifest = format!(
        "pool serial\n  depth = 1\n\
         rule run\n  command = {command}\n\
         build root: run\n\
         build pooled_later: run root\n  pool = serial\n\
         build normal: run root\n\
         build pooled_ready: run\n  pool = serial\n\
         build final: run pooled_later normal pooled_ready\n\
         default final\n"
    );
    for executable in std::env::var_os("KNIGHT_NINJA")
        .iter()
        .map(Path::new)
        .chain(std::iter::once(knight))
    {
        let work = tempdir().unwrap();
        fs::write(work.path().join("build.ninja"), &manifest).unwrap();
        let built = run(executable, work.path(), &["-j1"]);
        assert!(
            built.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&built.stdout),
            String::from_utf8_lossy(&built.stderr)
        );
        assert_eq!(
            fs::read_to_string(work.path().join("order.txt"))
                .unwrap()
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>(),
            ["root", "normal", "pooled_ready", "pooled_later", "final"],
            "executable={}",
            executable.display()
        );
    }
}

#[test]
fn delayed_pool_work_keeps_its_reservation_before_new_dependents() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule run\n  command = unused\n  description = DO $out\n",
            "pool serial\n  depth = 1\n",
            "build a: run source\n",
            "build b: run a\n  pool = serial\n",
            "build c: run source\n  pool = serial\n",
            "build d: run a c\n  pool = serial\n",
            "build e: run d\n",
            "build all: phony b e\n",
            "default all\n",
        ),
    )
    .unwrap();
    fs::write(temp.path().join("source"), "source").unwrap();

    let arguments = ["-n", "-j1"];
    let actual = run(knight, temp.path(), &arguments);
    assert!(actual.status.success());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        [
            "[1/5] DO a",
            "[2/5] DO c",
            "[3/5] DO b",
            "[4/5] DO d",
            "[5/5] DO e",
        ]
    );
    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
    }
}

#[test]
fn running_command_defers_phony_that_would_reserve_validation_pool() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let command = if cfg!(windows) {
        "cmd /d /c \"echo $out>>order.txt && echo built>$out\""
    } else {
        "printf '%s\\n' $out >> order.txt && touch $out"
    };
    let manifest = format!(
        "pool serial\n  depth = 1\n\
         rule run\n  command = {command}\n\
         build start: run\n\
         build gate: phony start\n\
         build runner: run start\n\
         build validation: run gate\n  pool = serial\n\
         build target: run runner |@ validation\n  pool = serial\n\
         default target\n"
    );

    for executable in std::env::var_os("KNIGHT_NINJA")
        .iter()
        .map(Path::new)
        .chain(std::iter::once(knight))
    {
        let work = tempdir().unwrap();
        fs::write(work.path().join("build.ninja"), &manifest).unwrap();
        let built = run(executable, work.path(), &["-j1", "--quiet"]);
        assert!(
            built.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&built.stdout),
            String::from_utf8_lossy(&built.stderr)
        );
        assert_eq!(
            fs::read_to_string(work.path().join("order.txt"))
                .unwrap()
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>(),
            ["start", "runner", "target", "validation"],
            "executable={}",
            executable.display()
        );
    }
}

#[test]
fn nan_load_limit_does_not_throttle_parallelism_like_ninja() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let command = if cfg!(windows) {
        concat!(
            "powershell -NoProfile -Command \"Set-Content '${out}.started' ready; ",
            "$$seen=$$false; for ($$i=0; $$i -lt 100; $$i++) { ",
            "if (Test-Path '${other}.started') { $$seen=$$true; break }; ",
            "Start-Sleep -Milliseconds 20 }; if (-not $$seen) { exit 9 }; ",
            "Set-Content '$out' built\"",
        )
    } else {
        concat!(
            "touch ${out}.started; i=0; ",
            "while test ! -f ${other}.started && test $$i -lt 100; ",
            "do sleep .02; i=$$((i+1)); done; ",
            "test -f ${other}.started && touch $out",
        )
    };
    let manifest = format!(
        "rule sync\n  command = {command}\n\
         build a: sync\n  other = b\n\
         build b: sync\n  other = a\n\
         default a b\n"
    );

    for executable in std::env::var_os("KNIGHT_NINJA")
        .iter()
        .map(Path::new)
        .chain(std::iter::once(knight))
    {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), &manifest).unwrap();
        let output = run(executable, temp.path(), &["-j2", "-l", "nan", "--quiet"]);
        assert!(
            output.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(temp.path().join("a").exists());
        assert!(temp.path().join("b").exists());
    }
}

#[test]
fn commands_that_expand_empty_match_ninjas_platform_behavior() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = "rule empty\n  command = $undefined\nbuild out: empty\ndefault out\n";

    for arguments in [&["-n"][..], &[][..]] {
        let mut observed = Vec::new();
        for executable in [Path::new(&ninja), knight] {
            let temp = tempdir().unwrap();
            fs::write(temp.path().join("build.ninja"), manifest).unwrap();
            observed.push(run(executable, temp.path(), arguments));
        }
        assert_eq!(observed[1].status.code(), observed[0].status.code());
        assert_eq!(observed[1].stdout, observed[0].stdout);
        let expected_error =
            String::from_utf8_lossy(&observed[0].stderr).replace("ninja:", "tool:");
        let actual_error = String::from_utf8_lossy(&observed[1].stderr).replace("knight:", "tool:");
        if cfg!(windows) && arguments.is_empty() {
            assert!(expected_error.contains("CreateProcess failed"));
            assert!(actual_error.contains("starting command '': empty command"));
        } else {
            assert_eq!(actual_error, expected_error);
        }
    }

    #[cfg(windows)]
    {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let alias = temp.path().join("ninja.exe");
        install_ninja_alias(knight, &alias);
        let expected = run(Path::new(&ninja), temp.path(), &[]);
        let actual = run(&alias, temp.path(), &[]);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
    }
}

#[test]
fn subprocess_command_start_failures_match_ninja_alias() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for command in if cfg!(windows) {
        ["ninja_no_such_command", "cmd /d /c ninja_no_such_command"]
    } else {
        ["ninja_no_such_command", "sh -c ninja_no_such_command"]
    } {
        let temp = tempdir().unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        install_ninja_alias(knight, &alias);
        fs::write(
            temp.path().join("build.ninja"),
            format!("rule fail\n  command = {command}\nbuild out: fail\ndefault out\n"),
        )
        .unwrap();
        let expected = run(Path::new(&ninja), temp.path(), &[]);
        let actual = run(&alias, temp.path(), &[]);
        assert_eq!(actual.status.code(), expected.status.code(), "{command}");
        assert_eq!(actual.stdout, expected.stdout, "{command}");
        assert_eq!(actual.stderr, expected.stderr, "{command}");
    }
}

#[cfg(unix)]
#[test]
fn subprocess_child_signal_statuses_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for signal in ["INT", "TERM", "HUP"] {
        let temp = tempdir().unwrap();
        let alias = temp.path().join("ninja");
        install_ninja_alias(knight, &alias);
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                "rule signal\n  command = kill -{signal} $$$$\nbuild out: signal\ndefault out\n"
            ),
        )
        .unwrap();
        let expected = run(Path::new(&ninja), temp.path(), &[]);
        let actual = run(&alias, temp.path(), &[]);
        assert_eq!(actual.status.code(), expected.status.code(), "{signal}");
        assert_eq!(actual.stdout, expected.stdout, "{signal}");
        assert_eq!(actual.stderr, expected.stderr, "{signal}");
    }
}

#[cfg(unix)]
#[test]
fn subprocess_parent_signal_statuses_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for signal in ["INT", "TERM", "HUP"] {
        let expected_dir = tempdir().unwrap();
        let actual_dir = tempdir().unwrap();
        let alias = actual_dir.path().join("ninja");
        install_ninja_alias(knight, &alias);
        let manifest = format!(
            "rule signal\n  command = kill -{signal} $$PPID; sleep 1\nbuild out: signal\ndefault out\n"
        );
        fs::write(expected_dir.path().join("build.ninja"), &manifest).unwrap();
        fs::write(actual_dir.path().join("build.ninja"), manifest).unwrap();
        let expected = run(Path::new(&ninja), expected_dir.path(), &[]);
        let actual = run(&alias, actual_dir.path(), &[]);
        assert_eq!(actual.status.code(), expected.status.code(), "{signal}");
        assert_eq!(actual.stdout, expected.stdout, "{signal}");
        assert_eq!(actual.stderr, expected.stderr, "{signal}");
    }
}

#[cfg(unix)]
#[test]
fn subprocess_closes_stdin_for_non_console_commands() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "rule read\n  command = cat -\nbuild out: read\ndefault out\n",
    )
    .unwrap();
    let expected = run(Path::new(&ninja), temp.path(), &[]);
    let actual = run(knight, temp.path(), &[]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[cfg(unix)]
#[test]
fn subprocess_set_supports_more_than_1024_parallel_processes() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    const PROCESS_COUNT: usize = 1025;
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let expected_dir = tempdir().unwrap();
    let actual_dir = tempdir().unwrap();
    let mut manifest = String::from("rule echo\n  command = /bin/echo\n");
    for index in 0..PROCESS_COUNT {
        manifest.push_str(&format!("build out{index}: echo\n"));
    }
    manifest.push_str("default");
    for index in 0..PROCESS_COUNT {
        manifest.push_str(&format!(" out{index}"));
    }
    manifest.push('\n');
    fs::write(expected_dir.path().join("build.ninja"), &manifest).unwrap();
    fs::write(actual_dir.path().join("build.ninja"), manifest).unwrap();
    let args = ["--quiet", "-j1025"];
    let expected = run(Path::new(&ninja), expected_dir.path(), &args);
    let actual = run(knight, actual_dir.path(), &args);
    assert!(
        actual.status.success(),
        "Knight failed: stdout={} stderr={}",
        String::from_utf8_lossy(&actual.stdout),
        String::from_utf8_lossy(&actual.stderr)
    );
    assert_eq!(actual.stdout, vec![b'\n'; PROCESS_COUNT]);
    if expected.status.success() {
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
    } else {
        #[cfg(not(any(target_os = "macos", target_os = "netbsd")))]
        panic!(
            "Ninja failed: stdout={} stderr={}",
            String::from_utf8_lossy(&expected.stdout),
            String::from_utf8_lossy(&expected.stderr)
        );
    }
}

#[cfg(windows)]
#[test]
fn command_path_separator_spelling_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    for (name, manifest) in [
        (
            "direct",
            "rule echo\n  command = echo [$in] [$in_newline] [$out]\nbuild out\\file: echo in\\file\n",
        ),
        (
            "first-node-spelling",
            "rule echo\n  command = echo [$in] [$out]\nbuild seed: phony dir/file\nbuild out\\file: echo dir\\file\n",
        ),
        (
            "variable-expanded",
            "rule echo\n  command = echo [$in] [$out]\ndir = nested\\dir\nbuild $dir\\out: echo $dir\\in\n",
        ),
        (
            "canonicalized",
            "rule echo\n  command = echo [$in] [$out]\nbuild old\\..\\final\\file: echo src\\part\\..\\file\n",
        ),
        (
            "mixed-separator-output",
            "rule echo\n  command = echo [$in] [$out]\nbuild out\\out2/out3\\out4: echo src\\in1\n",
        ),
        (
            "implicit-first-node-spelling",
            "rule echo\n  command = echo [$in] [$out]\nbuild seed: phony | dir/file\nbuild out\\file: echo dir\\file\n",
        ),
    ] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        for source in [
            "in/file",
            "dir/file",
            "nested/dir/in",
            "src/file",
            "src/in1",
        ] {
            let source = temp.path().join(source);
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::write(source, "source").unwrap();
        }
        for arguments in [&["-t", "commands"][..], &["-n", "-v"][..]] {
            let expected = run(ninja, temp.path(), arguments);
            let actual = run(knight, temp.path(), arguments);
            assert_eq!(actual.status.code(), expected.status.code(), "{name}");
            assert_eq!(actual.stdout, expected.stdout, "{name}");
            assert_eq!(actual.stderr, expected.stderr, "{name}");
        }
    }
}

#[test]
fn initial_pool_frontier_includes_clean_phony_dependents() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let arguments = ["-n", "-j1"];
    let prefix = concat!(
        "rule run\n  command = unused\n  description = DO $out\n",
        "pool serial\n  depth = 1\n",
        "build ready: run source\n  pool = serial\n",
        "build ph: phony source\n",
        "build later: run ph\n  pool = serial\n",
    );
    let cases = [
        (
            format!("{prefix}build after: run ready\nbuild all: phony later after\ndefault all\n"),
            ["[1/3] DO ready", "[2/3] DO later", "[3/3] DO after"],
        ),
        (
            format!("{prefix}build after: run later\nbuild all: phony ready after\ndefault all\n"),
            ["[1/3] DO later", "[2/3] DO ready", "[3/3] DO after"],
        ),
    ];

    for (manifest, order) in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("source"), "source").unwrap();
        let actual = run(knight, temp.path(), &arguments);
        assert!(actual.status.success());
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            order
        );
        if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
            let expected = run(Path::new(&ninja), temp.path(), &arguments);
            assert_eq!(actual.status.code(), expected.status.code());
            assert_eq!(actual.stdout, expected.stdout);
            assert_eq!(actual.stderr, expected.stderr);
        }
    }
}

#[test]
fn multi_output_notifications_reserve_pools_in_ninja_order() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let command = if cfg!(windows) {
        "cmd /d /c \"echo $out>>order.txt && echo built>$out\""
    } else {
        "printf '%s\\n' $out >> order.txt && touch $out"
    };
    let manifest = format!(
        "pool serial\n  depth = 1\n\
         rule run\n  command = {command}\n\
         build root | root.imp: run\n\
         build declared_first: run root.imp\n  pool = serial\n\
         build notified_first: run root\n  pool = serial\n\
         build final: run declared_first notified_first\n\
         default final\n"
    );
    for executable in std::env::var_os("KNIGHT_NINJA")
        .iter()
        .map(Path::new)
        .chain(std::iter::once(knight))
    {
        let work = tempdir().unwrap();
        fs::write(work.path().join("build.ninja"), &manifest).unwrap();
        assert!(run(executable, work.path(), &["-j1"]).status.success());
        assert_eq!(
            fs::read_to_string(work.path().join("order.txt"))
                .unwrap()
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>(),
            ["root", "notified_first", "declared_first", "final"],
            "executable={}",
            executable.display()
        );
    }
}

#[test]
fn status_total_excludes_clean_targets_with_dirty_validations() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let command = if cfg!(windows) {
        "cmd /d /c echo stamp > $out"
    } else {
        "printf stamp > $out"
    };
    fs::write(
        temp.path().join("build.ninja"),
        format!(
            "rule stamp\n  command = {command}\n\
             build validation | validation.imp: stamp\n\
             build target: stamp |@ validation.imp\n\
             default target\n"
        ),
    )
    .unwrap();

    for executable in std::env::var_os("KNIGHT_NINJA")
        .iter()
        .map(Path::new)
        .chain(std::iter::once(knight))
    {
        let work = tempdir().unwrap();
        fs::copy(
            temp.path().join("build.ninja"),
            work.path().join("build.ninja"),
        )
        .unwrap();
        assert!(run(executable, work.path(), &[]).status.success());
        let incremental = run(executable, work.path(), &[]);
        assert!(incremental.status.success());
        assert_eq!(
            String::from_utf8_lossy(&incremental.stdout)
                .lines()
                .collect::<Vec<_>>(),
            [format!("[1/1] {}", command.replace("$out", "validation"))],
            "executable={}",
            executable.display()
        );
    }
}

#[test]
fn phony_mtime_ignores_order_only_and_validation_inputs() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let command = if cfg!(windows) {
        "cmd /d /c echo stamp > $out"
    } else {
        "printf stamp > $out"
    };
    let manifest = format!(
        "rule stamp\n  command = {command}\n\
         build order | order.imp: stamp\n\
         build validation | validation.imp: stamp\n\
         build order_alias: phony source || order\n\
         build validation_alias: phony source |@ validation.imp\n\
         build order_consumer: stamp order_alias\n\
         build validation_consumer: stamp validation_alias\n\
         build all: phony order_consumer validation_consumer\n\
         default all\n"
    );
    for executable in std::env::var_os("KNIGHT_NINJA")
        .iter()
        .map(Path::new)
        .chain(std::iter::once(knight))
    {
        let work = tempdir().unwrap();
        fs::write(work.path().join("build.ninja"), &manifest).unwrap();
        fs::write(work.path().join("source"), "source").unwrap();
        assert!(run(executable, work.path(), &[]).status.success());
        let incremental = run(executable, work.path(), &[]);
        assert!(incremental.status.success());
        let stdout = String::from_utf8_lossy(&incremental.stdout);
        assert!(stdout.contains("order"), "stdout={stdout}");
        assert!(stdout.contains("validation"), "stdout={stdout}");
        assert!(!stdout.contains("consumer"), "stdout={stdout}");
        assert_eq!(stdout.lines().count(), 2, "stdout={stdout}");
    }
}

#[cfg(windows)]
#[test]
fn scheduler_prioritizes_the_longest_remaining_path_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let manifest = concat!(
        "rule r\n  command = cmd /d /c \"echo $out>>order.txt && echo built>$out\"\n",
        "build out: r a0 b0 c0\n",
        "build a0: r a1\n",
        "build a1: r a2\n",
        "build b0: r b1\n",
        "build c0: r b1\n",
        "default out\n",
    );
    let mut orders = Vec::new();
    for executable in [ninja, knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("a2"), "source").unwrap();
        fs::write(temp.path().join("b1"), "source").unwrap();
        let result = run(executable, temp.path(), &["-j1"]);
        assert!(
            result.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        orders.push(fs::read_to_string(temp.path().join("order.txt")).unwrap());
    }
    let normalize = |order: &str| {
        order
            .lines()
            .map(|line| line.trim().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(normalize(&orders[1]), normalize(&orders[0]));
    assert_eq!(normalize(&orders[1]), ["a1", "a0", "b0", "c0", "out"]);
}

#[cfg(windows)]
#[test]
fn explicit_status_format_matches_ninja_and_counts_only_dirty_edges() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule touch\n  command = cmd /d /c echo x>$out\n  description = BUILD $out\n",
            "build a: touch\nbuild b: touch\nbuild all: phony a b\ndefault all\n",
        ),
    )
    .unwrap();
    assert!(run(knight, temp.path(), &[]).status.success());
    fs::remove_file(temp.path().join("a")).unwrap();

    let arguments = [
        "-n",
        "--status",
        "$started,$finished,$running,$remaining,$total $description",
    ];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert!(expected.status.success() && actual.status.success());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[cfg(windows)]
#[test]
fn historical_status_prediction_matches_ninja_in_dry_run() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule touch\n  command = cmd /d /c echo x>$out\n  description = DO $out\n",
            "build a: touch\nbuild b: touch a\ndefault b\n",
        ),
    )
    .unwrap();
    fs::write(
        temp.path().join(".ninja_log"),
        "# ninja log v7\n0\t1000\t0\ta\t0\n0\t9000\t0\tb\t0\n",
    )
    .unwrap();

    let arguments = [
        "-n",
        "--status",
        "$predicted_progress|$progress|$eta_seconds $description",
    ];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert!(expected.status.success() && actual.status.success());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&expected.stdout)
            .lines()
            .collect::<Vec<_>>()
    );
}

#[cfg(windows)]
#[test]
fn ninja_and_knight_exchange_gcc_dependency_logs() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let manifest = concat!(
        "rule cc\n",
        "  command = cmd /d /c \"type $in > $out && echo $out: $in header.h > ${out}.d\"\n",
        "  deps = gcc\n",
        "  depfile = ${out}.d\n",
        "build out.o: cc source.c\n",
        "default out.o\n",
    );

    for (producer, consumer) in [(knight, ninja), (ninja, knight)] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("source.c"), "source\n").unwrap();
        fs::write(temp.path().join("header.h"), "header\n").unwrap();

        let built = run(producer, temp.path(), &[]);
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        assert!(temp.path().join(".ninja_deps").exists());
        assert!(!temp.path().join("out.o.d").exists());

        let deps_dump = run(knight, temp.path(), &["-t", "deps"]);
        let expected_query = run(ninja, temp.path(), &["-t", "query", "out.o"]);
        let actual_query = run(knight, temp.path(), &["-t", "query", "out.o"]);
        assert_eq!(
            String::from_utf8_lossy(&actual_query.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected_query.stdout)
                .lines()
                .collect::<Vec<_>>(),
            "producer={}",
            producer.display()
        );
        let noop = run(consumer, temp.path(), &[]);
        assert!(
            noop.status.success(),
            "{}",
            String::from_utf8_lossy(&noop.stderr)
        );
        assert!(
            String::from_utf8_lossy(&noop.stdout).contains("no work"),
            "producer={} consumer={} stdout={} stderr={} deps={}",
            producer.display(),
            consumer.display(),
            String::from_utf8_lossy(&noop.stdout),
            String::from_utf8_lossy(&noop.stderr),
            String::from_utf8_lossy(&deps_dump.stdout)
        );

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("header.h"), "changed\n").unwrap();
        let rebuilt = run(consumer, temp.path(), &[]);
        assert!(
            rebuilt.status.success(),
            "consumer={} stdout={} stderr={}",
            consumer.display(),
            String::from_utf8_lossy(&rebuilt.stdout),
            String::from_utf8_lossy(&rebuilt.stderr)
        );
        assert!(!String::from_utf8_lossy(&rebuilt.stdout).contains("no work"));
    }
}

#[cfg(windows)]
#[test]
fn manifest_dirty_edges_do_not_load_stale_discovered_dependencies() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = concat!(
        "rule make_out\n",
        "  command = cmd /d /c \"echo out>>commands.txt & copy /y in out >nul\"\n",
        "rule make_validation\n",
        "  command = cmd /d /c \"echo validate>>commands.txt & copy /y in2 validate >nul\"\n",
        "rule make_out2\n",
        "  command = cmd /d /c \"echo out2>>commands.txt & copy /y in3 out2 >nul & echo out2: out > out2.d\"\n",
        "  deps = gcc\n",
        "  depfile = out2.d\n",
        "build out: make_out in |@ validate\n",
        "build validate: make_validation in2 | out\n",
        "build out2: make_out2 in3\n",
        "default out2\n",
    );

    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        for input in ["in", "in2", "in3"] {
            fs::write(temp.path().join(input), input).unwrap();
        }
        fs::write(temp.path().join("out2.d"), "out2: out\n").unwrap();

        let first = run(executable, temp.path(), &["-j1"]);
        assert!(
            first.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("commands.txt"))
                .unwrap()
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>(),
            ["out2"],
            "first build with {}",
            executable.display()
        );

        fs::write(temp.path().join("commands.txt"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("in"), "new in").unwrap();
        fs::write(temp.path().join("in2"), "new in2").unwrap();
        let discovered = run(executable, temp.path(), &["-j1"]);
        assert!(discovered.status.success());
        let commands = fs::read_to_string(temp.path().join("commands.txt")).unwrap();
        assert_eq!(commands.lines().count(), 3, "commands={commands:?}");
        assert_eq!(
            commands.lines().next().map(str::trim),
            Some("out"),
            "commands={commands:?}"
        );

        fs::write(temp.path().join("commands.txt"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("in2"), "newer in2").unwrap();
        fs::write(temp.path().join("in3"), "newer in3").unwrap();
        let manifest_dirty = run(executable, temp.path(), &["-j1"]);
        assert!(manifest_dirty.status.success());
        assert_eq!(
            fs::read_to_string(temp.path().join("commands.txt"))
                .unwrap()
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>(),
            ["out2"],
            "manifest-dirty build with {}",
            executable.display()
        );

        fs::write(temp.path().join("commands.txt"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("in2"), "newest in2").unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            manifest.replace("echo out2>>commands.txt", "echo changed>>commands.txt"),
        )
        .unwrap();
        let command_dirty = run(executable, temp.path(), &["-j1"]);
        assert!(command_dirty.status.success());
        assert_eq!(
            fs::read_to_string(temp.path().join("commands.txt"))
                .unwrap()
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>(),
            ["changed"],
            "command-dirty build with {}",
            executable.display()
        );
    }
}

#[cfg(windows)]
#[test]
fn stale_depfile_cycle_is_ignored_when_declared_inputs_are_dirty() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = concat!(
        "rule copy\n",
        "  command = cmd /d /c \"copy /y $in $out >nul\"\n",
        "rule copy_deps\n",
        "  command = cmd /d /c \"copy /y $in $out >nul & echo b: X > d.d\"\n",
        "  depfile = d.d\n",
        "build b: copy_deps a\n",
        "build c: copy b\n",
        "build d: copy c\n",
        "default d\n",
    );

    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("a"), "first").unwrap();
        fs::write(temp.path().join("X"), "dependency").unwrap();
        assert!(run(executable, temp.path(), &[]).status.success());

        fs::write(temp.path().join("d.d"), "b: d\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("a"), "second").unwrap();
        let rebuilt = run(executable, temp.path(), &[]);
        assert!(
            rebuilt.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&rebuilt.stdout),
            String::from_utf8_lossy(&rebuilt.stderr)
        );
        assert_eq!(fs::read_to_string(temp.path().join("d")).unwrap(), "second");
    }
}

#[cfg(windows)]
#[test]
fn deps_tool_without_targets_uses_dependency_log_node_order_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n",
            "  command = cmd /d /c \"echo object>$out && echo $out$: $in > ${out}.d\"\n",
            "  deps = gcc\n  depfile = ${out}.d\n",
            "build z.o: cc z.c\n",
            "build a.o: cc a.c\n",
            "build all: phony z.o a.o\ndefault all\n",
        ),
    )
    .unwrap();
    fs::write(temp.path().join("z.c"), "z").unwrap();
    fs::write(temp.path().join("a.c"), "a").unwrap();
    assert!(run(knight, temp.path(), &[]).status.success());

    let expected = run(ninja, temp.path(), &["-t", "deps"]);
    let actual = run(knight, temp.path(), &["-t", "deps"]);
    assert!(expected.status.success() && actual.status.success());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&expected.stdout)
            .lines()
            .collect::<Vec<_>>()
    );
}

#[cfg(windows)]
#[test]
fn stale_depfile_failures_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let manifest = concat!(
        "rule cc\n",
        "  command = cmd /d /c \"copy /y source out >nul && echo out$: source > out.d\"\n",
        "  depfile = out.d\n",
        "build out: cc source\n",
        "default out\n",
    );

    for (depfile, expected_code, expected_fragment) in [
        ("out: missing.h\n", 0, ""),
        (
            "out undeclared: source\n",
            1,
            "depfile mentions 'undeclared' as an output",
        ),
        ("not a depfile\n", 1, "expected ':' in depfile"),
    ] {
        for executable in [ninja, knight] {
            let temp = tempdir().unwrap();
            fs::write(temp.path().join("build.ninja"), manifest).unwrap();
            fs::write(temp.path().join("source"), "source").unwrap();
            let initial = run(executable, temp.path(), &[]);
            assert!(
                initial.status.success(),
                "executable={} stdout={} stderr={}",
                executable.display(),
                String::from_utf8_lossy(&initial.stdout),
                String::from_utf8_lossy(&initial.stderr)
            );
            fs::write(temp.path().join("out.d"), depfile).unwrap();
            let result = run(executable, temp.path(), &[]);
            assert_eq!(
                result.status.code(),
                Some(expected_code),
                "executable={executable:?} depfile={depfile:?} stdout={} stderr={}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            if !expected_fragment.is_empty() {
                assert!(
                    String::from_utf8_lossy(&result.stderr).contains(expected_fragment),
                    "executable={} stdout={} stderr={}",
                    executable.display(),
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn restat_does_not_hide_a_missing_downstream_depfile() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = concat!(
        "rule noop\n",
        "  command = cmd /d /c exit 0\n",
        "  restat = 1\n",
        "rule cc\n",
        "  command = cmd /d /c \"echo run>>count.txt && copy /y header.h out >nul && echo out$: header.h > out.d\"\n",
        "  depfile = out.d\n",
        "build header.h: noop header.in\n",
        "build out: cc header.h\n",
        "default out\n",
    );

    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("header.in"), "initial").unwrap();
        fs::write(temp.path().join("header.h"), "unchanged").unwrap();
        assert!(run(executable, temp.path(), &[]).status.success());
        assert_eq!(
            fs::read_to_string(temp.path().join("count.txt"))
                .unwrap()
                .lines()
                .count(),
            1
        );

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("header.in"), "changed once").unwrap();
        assert!(run(executable, temp.path(), &[]).status.success());
        assert_eq!(
            fs::read_to_string(temp.path().join("count.txt"))
                .unwrap()
                .lines()
                .count(),
            1
        );

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("header.in"), "changed twice").unwrap();
        fs::remove_file(temp.path().join("out.d")).unwrap();
        let rebuilt = run(executable, temp.path(), &[]);
        assert!(
            rebuilt.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&rebuilt.stdout),
            String::from_utf8_lossy(&rebuilt.stderr)
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("count.txt"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
}

#[cfg(windows)]
#[test]
fn unchanged_restat_output_becomes_clean_after_the_command_runs() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = concat!(
        "rule stable\n",
        "  command = cmd /d /c \"echo run>>runs.txt && if not exist out echo stable>out\"\n",
        "  restat = 1\n",
        "build out: stable input\n",
        "default out\n",
    );

    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("input"), "initial").unwrap();
        assert!(run(executable, temp.path(), &[]).status.success());

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("input"), "changed").unwrap();
        assert!(run(executable, temp.path(), &[]).status.success());
        let no_work = run(executable, temp.path(), &[]);
        assert!(no_work.status.success());
        assert!(
            String::from_utf8_lossy(&no_work.stdout).contains("no work"),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&no_work.stdout),
            String::from_utf8_lossy(&no_work.stderr)
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("runs.txt"))
                .unwrap()
                .lines()
                .count(),
            2,
            "executable={}",
            executable.display()
        );
    }
}

#[cfg(windows)]
#[test]
fn missing_to_missing_restat_output_does_not_dirty_its_dependent() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = concat!(
        "rule noop\n  command = cmd /d /c \"echo noop>>runs.txt\"\n  restat = 1\n",
        "rule consume\n  command = cmd /d /c \"echo consume>>runs.txt && echo output>$out\"\n",
        "build absent: noop input\n",
        "build final: consume absent\n",
        "default final\n",
    );

    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("input"), "initial").unwrap();
        let first = run(executable, temp.path(), &[]);
        assert!(
            first.status.success(),
            "executable={}",
            executable.display()
        );

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("input"), "changed").unwrap();
        let second = run(executable, temp.path(), &[]);
        assert!(
            second.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&second.stdout),
            String::from_utf8_lossy(&second.stderr)
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("runs.txt"))
                .unwrap()
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>(),
            ["noop", "consume", "noop"],
            "executable={}",
            executable.display()
        );
        assert!(!temp.path().join("absent").exists());
    }
}

#[cfg(windows)]
#[test]
fn knight_tracks_and_filters_msvc_showincludes() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n",
            "  command = powershell -NoProfile -Command \"Write-Output 'source.c'; ",
            "Write-Output 'Note: including file: header.h'; ",
            "Write-Output 'warning: kept'; Set-Content -NoNewline -Path '$out' -Value object\"\n",
            "  description = CC $out\n",
            "  deps = msvc\n",
            "build out.obj: cc source.c\n",
            "default out.obj\n",
        ),
    )
    .unwrap();
    fs::write(temp.path().join("source.c"), "source\n").unwrap();
    fs::write(temp.path().join("header.h"), "header\n").unwrap();

    let first = run(knight, temp.path(), &[]);
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(first_stdout.contains("warning: kept"));
    assert!(!first_stdout.contains("including file"));

    let noop = run(knight, temp.path(), &[]);
    assert!(String::from_utf8_lossy(&noop.stdout).contains("no work"));

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(temp.path().join("header.h"), "changed\n").unwrap();
    let rebuilt = run(knight, temp.path(), &["-d", "explain"]);
    assert!(
        rebuilt.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rebuilt.stderr).contains("discovered input header.h"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rebuilt.stdout),
        String::from_utf8_lossy(&rebuilt.stderr)
    );
}

#[cfg(windows)]
#[test]
fn msvc_output_line_boundaries_match_ninja_byte_for_byte() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let manifest = concat!(
        "rule cc\n",
        "  command = python -c \"import sys; sys.stdout.buffer.write(b'source.c\\rkept one\\r\\nNote: including file: \\nNote: including file:   \\rkept two\\nNote: including file: header.h'); open(r'$out', 'wb').write(b'x')\"\n",
        "  description = CC $out\n",
        "  deps = msvc\n",
        "build out.obj: cc source.c\n",
        "default out.obj\n",
    );
    let run_build = |name: &str, executable: &Path| {
        let directory = temp.path().join(name);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("build.ninja"), manifest).unwrap();
        fs::write(directory.join("source.c"), "source\n").unwrap();
        fs::write(directory.join("header.h"), "header\n").unwrap();
        run(executable, &directory, &[])
    };

    let expected = run_build("ninja", Path::new(&ninja));
    let actual = run_build("knight", knight);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn subninja_rules_and_variables_remain_file_scoped() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "x = parent\n",
            "inherited = visible\n",
            "rule shared\n  command = echo parent-$x\n",
            "subninja child.ninja\n",
            "build parent: shared\n",
            "default parent child inherited_target\n",
        ),
    )
    .unwrap();
    fs::write(
        temp.path().join("child.ninja"),
        concat!(
            "x = child\n",
            "rule shared\n  command = echo child-$x\n",
            "rule child_rule\n  command = echo ${x}-${inherited}\n",
            "build child: shared\n",
            "build inherited_target: child_rule\n",
        ),
    )
    .unwrap();

    let output = run(knight, temp.path(), &["-n", "-v"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("echo parent-parent"), "{stdout}");
    assert!(stdout.contains("echo child-child"), "{stdout}");
    assert!(stdout.contains("echo child-visible"), "{stdout}");
}

#[test]
fn upstream_include_and_subninja_scope_cases_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    let include = tempdir().unwrap();
    fs::write(
        include.path().join("build.ninja"),
        concat!(
            "var = outer\n",
            "include child.ninja\n",
            "rule show\n  command = echo $var\n",
            "build out: show\n",
            "default out\n",
        ),
    )
    .unwrap();
    fs::write(include.path().join("child.ninja"), "var = inner\n").unwrap();
    let expected = run(ninja, include.path(), &["-n", "-v"]);
    let actual = run(knight, include.path(), &["-n", "-v"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);

    let subninja = tempdir().unwrap();
    fs::write(
        subninja.path().join("build.ninja"),
        concat!(
            "include rules.ninja\n",
            "subninja child.ninja\n",
            "build parent: cat\n",
            "default parent child\n",
        ),
    )
    .unwrap();
    fs::write(
        subninja.path().join("rules.ninja"),
        "rule cat\n  command = echo cat\n",
    )
    .unwrap();
    fs::write(
        subninja.path().join("child.ninja"),
        "include rules.ninja\nbuild child: cat\n",
    )
    .unwrap();
    let expected = run(ninja, subninja.path(), &["-n", "-v"]);
    let actual = run(knight, subninja.path(), &["-n", "-v"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn dependency_type_configuration_matches_ninjas_build_phases() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let alias = temp
        .path()
        .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
    install_ninja_alias(knight, &alias);

    let unknown = temp.path().join("unknown");
    fs::create_dir(&unknown).unwrap();
    fs::write(
        unknown.join("build.ninja"),
        concat!(
            "rule good\n  command = echo good\n",
            "rule bad\n  command = echo bad\n  deps = weird\n",
            "build okay: good\n",
            "build broken: bad\n",
            "default okay\n",
        ),
    )
    .unwrap();
    assert!(run(knight, &unknown, &["-n"]).status.success());
    assert!(
        run(knight, &unknown, &["-t", "commands", "broken"])
            .status
            .success()
    );
    for arguments in [&["-n", "broken"][..], &["-n", "--quiet", "broken"][..]] {
        let expected = run(Path::new(&ninja), &unknown, arguments);
        let actual = run(&alias, &unknown, arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
    }

    let gcc = temp.path().join("gcc");
    fs::create_dir(&gcc).unwrap();
    fs::write(
        gcc.join("build.ninja"),
        "rule cc\n  command = echo compile\n  deps = gcc\nbuild out: cc\n",
    )
    .unwrap();
    for arguments in [&["-n"][..], &["-n", "--quiet"][..], &["-n", "-k0"][..]] {
        let expected = run(Path::new(&ninja), &gcc, arguments);
        let actual = run(&alias, &gcc, arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
    }
}

#[test]
fn eager_variable_values_remain_literal_during_rule_expansion() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "name = expanded\n",
            "literal = $$name\n",
            "rule echo\n  command = echo $value\n",
            "build out: echo\n  value = $literal\n",
            "default out\n",
        ),
    )
    .unwrap();

    let arguments = ["-n", "-v"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&expected.stdout).replace("\r\n", "\n")
    );
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn ninja_114_variable_names_and_newline_escape_match() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "ninja_required_version = 1.14\n",
            "with-dash = dash\n",
            "with.dot = dot\n",
            "rule echo\n  command = echo $with-dash-${with.dot}$^echo second\n",
            "build out: echo\n",
            "default out\n",
        ),
    )
    .unwrap();

    let arguments = ["-n", "-v"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&expected.stdout).replace("\r\n", "\n")
    );
    assert_eq!(actual.stderr, expected.stderr);
}

#[cfg(unix)]
#[test]
fn epoch_timestamp_is_recorded_as_one_in_deps_log_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            "rule epoch\n  command = touch $out && python3 -c \"import os; os.utime('out', (0, 0))\" && printf 'out:' > out.d\n  deps = gcc\n  depfile = out.d\nbuild out: epoch\n",
        )
        .unwrap();
        let result = run(executable, temp.path(), &[]);
        assert!(result.status.success(), "{}", executable.display());
        let deps = run(executable, temp.path(), &["-t", "deps", "out"]);
        assert!(deps.status.success(), "{}", executable.display());
        assert!(
            String::from_utf8_lossy(&deps.stdout).contains("deps mtime 1 (VALID)"),
            "{}: {}",
            executable.display(),
            String::from_utf8_lossy(&deps.stdout)
        );
    }
}

#[test]
fn comments_and_implicit_only_outputs_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "suffix = value # literal\n",
            "  # indented comment\n",
            "rule echo\n  command = echo $suffix\n",
            "build | implicit: echo\n",
            "default implicit\n",
        ),
    )
    .unwrap();
    let arguments = ["-n", "-v"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&expected.stdout).replace("\r\n", "\n")
    );
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn unknown_target_suggestions_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "build application: phony source\n",
    )
    .unwrap();
    for target in ["applicaton", "clean", "help"] {
        let expected = run(Path::new(&ninja), temp.path(), &[target]);
        let actual = run(knight, temp.path(), &[target]);
        assert_eq!(actual.status.code(), expected.status.code());
        let expected = String::from_utf8_lossy(&expected.stderr);
        let actual = String::from_utf8_lossy(&actual.stderr);
        let expected_message = expected.strip_prefix("ninja: error: ").unwrap().trim();
        let actual_message = actual.strip_prefix("knight: error: ").unwrap().trim();
        assert_eq!(actual_message, expected_message, "target={target}");
    }
}

#[cfg(windows)]
#[test]
fn knight_builds_generated_dyndeps_before_dynamic_inputs() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule scan\n",
            "  command = powershell -NoProfile -Command \"Set-Content -Path deps.dd ",
            "-Value @('ninja_dyndep_version = 1','build generated.txt | module.txt: dyndep',",
            "'build out.txt: dyndep | module.txt')\"\n",
            "rule generate\n  command = cmd /d /c \"echo generated>generated.txt ",
            "&& echo module>module.txt\"\n",
            "rule consume\n  command = cmd /d /c type module.txt >$out\n",
            "build deps.dd: scan\n",
            "build generated.txt: generate || deps.dd\n",
            "  dyndep = deps.dd\n",
            "build out.txt: consume || deps.dd\n",
            "  dyndep = deps.dd\n",
            "default out.txt\n",
        ),
    )
    .unwrap();

    let first = run(knight, temp.path(), &[]);
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let progress = String::from_utf8_lossy(&first.stdout);
    for status in ["[1/3]", "[2/3]", "[3/3]"] {
        assert!(progress.contains(status), "missing {status} in {progress}");
    }
    assert!(temp.path().join("deps.dd").exists());
    assert!(temp.path().join("generated.txt").exists());
    assert!(temp.path().join("module.txt").exists());
    assert_eq!(
        fs::read_to_string(temp.path().join("out.txt"))
            .unwrap()
            .trim(),
        "module"
    );

    let noop = run(knight, temp.path(), &["-d", "explain"]);
    assert!(
        String::from_utf8_lossy(&noop.stdout).contains("no work"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&noop.stdout),
        String::from_utf8_lossy(&noop.stderr)
    );

    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        let upstream = run(Path::new(&ninja), temp.path(), &[]);
        assert!(
            upstream.status.success()
                && String::from_utf8_lossy(&upstream.stdout).contains("no work"),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&upstream.stdout),
            String::from_utf8_lossy(&upstream.stderr)
        );
    }
}

#[cfg(windows)]
#[test]
fn two_level_dyndep_discovery_reaches_a_fixed_point() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let manifest = concat!(
        "rule copy\n  command = cmd /d /c \"echo $out>>order.txt && copy /y $in $out >nul\"\n",
        "rule touch\n  command = cmd /d /c \"echo $out>>order.txt && echo built>$out\"\n",
        "build dd0: copy dd0-in\n",
        "build dd1: copy dd1-in\n",
        "build source: touch\n",
        "build middle: touch || dd0\n  dyndep = dd0\n",
        "build final: touch || dd1\n  dyndep = dd1\n",
        "default final\n",
    );
    let dd1 = "ninja_dyndep_version = 1\nbuild final: dyndep | middle\n";
    let dd0 = "ninja_dyndep_version = 1\nbuild middle: dyndep | source\n";
    let mut orders = Vec::new();

    for executable in [ninja, knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("dd1-in"), dd1).unwrap();
        fs::write(temp.path().join("dd0-in"), dd0).unwrap();
        fs::write(temp.path().join("final"), "old").unwrap();
        let result = run(executable, temp.path(), &["-j1"]);
        assert!(
            result.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        orders.push(
            fs::read_to_string(temp.path().join("order.txt"))
                .unwrap()
                .lines()
                .map(|line| line.trim().to_owned())
                .collect::<Vec<_>>(),
        );
        for output in ["dd0", "dd1", "source", "middle", "final"] {
            assert!(temp.path().join(output).exists(), "output={output}");
        }
    }
    assert_eq!(orders[1], orders[0]);
    assert_eq!(orders[1], ["dd1", "dd0", "source", "middle", "final"]);
}

#[test]
fn ready_dyndep_outputs_are_loaded_before_missing_input_validation() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule run\n  command = echo run\n",
            "build out: run in || dd\n  dyndep = dd\n",
            "build in: run circ\n",
        ),
    )
    .unwrap();
    fs::write(
        temp.path().join("dd"),
        "ninja_dyndep_version = 1\nbuild out | circ: dyndep\n",
    )
    .unwrap();

    let expected = run(ninja, temp.path(), &["-n", "out"]);
    let actual = run(knight, temp.path(), &["-n", "out"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    let expected_error = String::from_utf8_lossy(&expected.stderr)
        .replace('\r', "")
        .replace("ninja:", "tool:");
    let actual_error = String::from_utf8_lossy(&actual.stderr)
        .replace('\r', "")
        .replace("knight:", "tool:");
    assert_eq!(actual_error, expected_error);
}

#[cfg(windows)]
#[test]
fn generated_dyndep_keeps_independent_requested_work_concurrent() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = concat!(
        "rule make_out1\n",
        "  command = powershell -NoProfile -Command \"Set-Content out1.started ready; ",
        "$$seen=$$false; for ($$i=0; $$i -lt 100; $$i++) { ",
        "if (Test-Path zdd.started) { $$seen=$$true; break }; ",
        "Start-Sleep -Milliseconds 20 }; if (-not $$seen) { exit 9 }; ",
        "Set-Content out1 built; Set-Content out1.imp built\"\n",
        "rule make_dyndep\n",
        "  command = powershell -NoProfile -Command \"Set-Content zdd.started ready; ",
        "$$seen=$$false; for ($$i=0; $$i -lt 100; $$i++) { ",
        "if (Test-Path out1.started) { $$seen=$$true; break }; ",
        "Start-Sleep -Milliseconds 20 }; if (-not $$seen) { exit 9 }; ",
        "Copy-Item zdd-in zdd\"\n",
        "rule copy\n  command = cmd /d /c copy /y $in $out >nul\n",
        "build out1 | out1.imp: make_out1\n",
        "build zdd: make_dyndep zdd-in\n",
        "build out2: copy out1 || zdd\n  dyndep = zdd\n",
        "default out1 out2\n",
    );
    let dyndep = "ninja_dyndep_version = 1\nbuild out2: dyndep | out1.imp\n";

    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("zdd-in"), dyndep).unwrap();
        let result = run(executable, temp.path(), &["-j2"]);
        assert!(
            result.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&result.stdout)
                .lines()
                .filter(|line| line.starts_with('['))
                .count(),
            3,
            "unexpected progress output from {}: {}",
            executable.display(),
            String::from_utf8_lossy(&result.stdout)
        );
        for output in ["out1", "out1.imp", "zdd", "out2"] {
            assert!(temp.path().join(output).exists(), "missing {output}");
        }
    }
}

#[test]
fn generated_dyndep_dry_run_lists_safe_work_once() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule run\n  command = echo $out\n",
            "build independent: run\n",
            "build dd: run dd-in\n",
            "build out: run || dd\n  dyndep = dd\n",
            "default independent out\n",
        ),
    )
    .unwrap();
    fs::write(temp.path().join("dd-in"), "source").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(
        temp.path().join("dd"),
        "ninja_dyndep_version = 1\nbuild out: dyndep | independent\n",
    )
    .unwrap();

    let expected = run(Path::new(&ninja), temp.path(), &["-n", "-j2"]);
    let actual = run(knight, temp.path(), &["-n", "-j2"]);
    assert!(
        expected.status.success() && actual.status.success(),
        "ninja_stdout={} ninja_stderr={} knight_stdout={} knight_stderr={}",
        String::from_utf8_lossy(&expected.stdout),
        String::from_utf8_lossy(&expected.stderr),
        String::from_utf8_lossy(&actual.stdout),
        String::from_utf8_lossy(&actual.stderr)
    );
    for output in ["independent", "dd", "out"] {
        let command = format!("echo {output}");
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .filter(|line| line.ends_with(&command))
                .count(),
            1,
            "command={command:?} stdout={}",
            String::from_utf8_lossy(&actual.stdout)
        );
    }
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout).lines().count(),
        String::from_utf8_lossy(&expected.stdout).lines().count()
    );
}

#[cfg(windows)]
#[test]
fn tool_target_modes_and_rule_descriptions_match_ninja() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n  command = echo cc\n  description = compile $out\n",
            "rule link\n  command = echo link\n",
            "build a.o: cc a.c\n",
            "build b.o: cc b.c\n",
            "build app: link a.o b.o\n",
            "default app\n",
        ),
    )
    .unwrap();

    let cases: &[&[&str]] = &[
        &["-t", "targets"],
        &["-t", "targets", "depth", "2"],
        &["-t", "targets", "depth", "0"],
        &["-t", "targets", "depth", "not-a-number"],
        &["-t", "targets", "all"],
        &["-t", "targets", "rule"],
        &["-t", "targets", "rule", "cc"],
        &["-t", "rules", "-d"],
        &["-t", "wincodepage"],
    ];
    for arguments in cases {
        let actual = run(knight, temp.path(), arguments);
        assert!(actual.status.success(), "arguments={arguments:?}");
        if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
            let expected = run(Path::new(&ninja), temp.path(), arguments);
            assert!(expected.status.success(), "arguments={arguments:?}");
            assert_eq!(
                String::from_utf8_lossy(&actual.stdout)
                    .lines()
                    .collect::<Vec<_>>(),
                String::from_utf8_lossy(&expected.stdout)
                    .lines()
                    .collect::<Vec<_>>(),
                "arguments={arguments:?}"
            );
        }
    }

    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        let arguments = ["-t", "wincodepage"];
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        let actual = run(knight, temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
    }
}

#[cfg(windows)]
#[test]
fn early_tools_do_not_require_a_manifest() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    for arguments in [
        &["-t", "list"][..],
        &["-t", "urtle"][..],
        &["-t", "wincodepage"][..],
        &["-d", "list"][..],
        &["-w", "list"][..],
    ] {
        let actual = run(knight, temp.path(), arguments);
        if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
            let expected = run(Path::new(&ninja), temp.path(), arguments);
            assert_eq!(actual.status.code(), expected.status.code());
            assert_eq!(
                String::from_utf8_lossy(&actual.stdout)
                    .lines()
                    .collect::<Vec<_>>(),
                String::from_utf8_lossy(&expected.stdout)
                    .lines()
                    .collect::<Vec<_>>(),
                "arguments={arguments:?}"
            );
        } else if !matches!(arguments, &["-d", "list"] | &["-w", "list"]) {
            assert!(actual.status.success(), "arguments={arguments:?}");
        }
    }
    assert!(run(knight, temp.path(), &["-t", "restat"]).status.success());

    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        let arguments = ["-t", "wincodepage", "unexpected"];
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        let actual = run(knight, temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>()
        );

        let expected = run(Path::new(&ninja), temp.path(), &["-t", "clena"]);
        let actual = run(knight, temp.path(), &["-t", "clena"]);
        assert_eq!(actual.status.code(), expected.status.code());
        let expected = String::from_utf8_lossy(&expected.stderr);
        let actual = String::from_utf8_lossy(&actual.stderr);
        assert_eq!(
            actual.strip_prefix("knight: error: ").unwrap().trim(),
            expected.strip_prefix("ninja: fatal: ").unwrap().trim()
        );

        for arguments in [
            &["-d", "stat"][..],
            &["-w", "phonycycle=war"][..],
            &["-t", "targets", "rul"][..],
        ] {
            fs::write(temp.path().join("build.ninja"), "build out: phony\n").unwrap();
            let expected = run(Path::new(&ninja), temp.path(), arguments);
            let actual = run(knight, temp.path(), arguments);
            assert_eq!(actual.status.code(), expected.status.code());
            let expected = String::from_utf8_lossy(&expected.stderr);
            let actual = String::from_utf8_lossy(&actual.stderr);
            let expected = expected
                .strip_prefix("ninja: error: ")
                .unwrap_or(&expected)
                .trim();
            let actual = actual
                .strip_prefix("knight: error: ")
                .unwrap_or(&actual)
                .trim();
            assert_eq!(actual, expected, "arguments={arguments:?}");
        }
    }
}

#[cfg(windows)]
#[test]
fn deprecated_msvc_helper_filters_output_and_writes_depfile() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let arguments = [
        "-t",
        "msvc",
        "-o",
        "obj",
        "-p",
        "INC: ",
        "--",
        "cmd",
        "/d",
        "/c",
        "echo source.c&&echo INC: header.h&&echo warning",
    ];
    let actual_dir = tempdir().unwrap();
    let actual = run(knight, actual_dir.path(), &arguments);
    assert!(
        actual.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&actual.stdout),
        String::from_utf8_lossy(&actual.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        ["warning"]
    );
    assert_eq!(
        fs::read_to_string(actual_dir.path().join("obj.d")).unwrap(),
        "obj: header.h\r\n"
    );
}

#[cfg(windows)]
#[test]
fn msvc_helper_normalizes_include_paths_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let absolute = temp
        .path()
        .join("Sub")
        .join("Header.h")
        .to_string_lossy()
        .to_uppercase();
    for include in [
        r"a\..\b".to_owned(),
        r"a\.\b".to_owned(),
        absolute,
        r"P:\vs08\..\wee\stuff.h".to_owned(),
    ] {
        let command = format!("echo Note: including file: {include}");
        let arguments = ["-t", "msvc", "-o", "obj", "--", "cmd", "/d", "/c", &command];
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        let expected_depfile = fs::read(temp.path().join("obj.d")).unwrap();
        fs::remove_file(temp.path().join("obj.d")).unwrap();
        let actual = run(knight, temp.path(), &arguments);
        let actual_depfile = fs::read(temp.path().join("obj.d")).unwrap();
        assert_eq!(actual.status.code(), expected.status.code(), "{include}");
        assert_eq!(actual_depfile, expected_depfile, "{include}");
    }
}

#[cfg(windows)]
#[test]
fn msvc_helper_alias_preserves_ninjas_max_path_rejection() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let alias = temp.path().join("ninja.exe");
    install_ninja_alias(knight, &alias);
    let include = (0..260)
        .map(|index| {
            if index % 10 == 4 && index < 259 {
                '\\'
            } else {
                'a'
            }
        })
        .collect::<String>();
    let command = format!("echo Note: including file: {include}");
    let arguments = ["-t", "msvc", "-o", "obj", "--", "cmd", "/d", "/c", &command];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(&alias, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);

    let native = run(knight, temp.path(), &arguments);
    assert!(native.status.success());
    assert!(
        fs::read_to_string(temp.path().join("obj.d"))
            .unwrap()
            .starts_with("obj: ")
    );
}

#[cfg(windows)]
#[test]
fn msvc_helper_environment_and_stderr_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("environment.bin"), b"foo=bar\0\0").unwrap();

    for arguments in [
        &[
            "-t",
            "msvc",
            "-e",
            "environment.bin",
            "--",
            "cmd",
            "/d",
            "/c",
            "echo foo is %foo%",
        ][..],
        &[
            "-t",
            "msvc",
            "--",
            "cmd",
            "/d",
            "/c",
            "echo to stdout&& echo to stderr 1>&2",
        ][..],
    ] {
        let expected = run(Path::new(&ninja), temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        assert_eq!(actual.stdout, expected.stdout, "{arguments:?}");
        assert_eq!(actual.stderr, expected.stderr, "{arguments:?}");
    }
}

#[cfg(windows)]
#[test]
fn deprecated_msvc_helper_options_match_ninja_getopt() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    for arguments in [
        &["-t", "msvc", "-h"][..],
        &["-t", "msvc", "--help=ignored"][..],
        &["-t", "msvc", "-x"][..],
        &["-t", "msvc", "--bogus"][..],
        &["-t", "msvc", "-o"][..],
    ] {
        let expected = run(Path::new(&ninja), temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>(),
            "{arguments:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&actual.stderr)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stderr)
                .lines()
                .collect::<Vec<_>>(),
            "{arguments:?}"
        );
    }
}

#[cfg(windows)]
#[test]
fn missingdeps_matches_ninja_output_and_exit_status() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule gen\n  command = cmd /d /c \"echo header>$out\"\n",
            "rule cc\n  command = cmd /d /c \"echo object>$out && echo $out$: generated.h>${out}.d\"\n",
            "  depfile = ${out}.d\n  deps = gcc\n",
            "build generated.h: gen\n",
            "build out: cc\n",
            "default generated.h out\n",
        ),
    )
    .unwrap();
    let built = run(knight, temp.path(), &[]);
    assert!(
        built.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );

    let arguments = &["-t", "missingdeps", "out"];
    let actual = run(knight, temp.path(), arguments);
    assert_eq!(actual.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&actual.stdout).contains("Missing dep: out uses generated.h"));
    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        let expected = run(Path::new(&ninja), temp.path(), arguments);
        assert_eq!(expected.status.code(), Some(3));
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn missingdeps_without_targets_scans_only_default_closures_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let mut expected_default = None;
    let python = if cfg!(windows) { "python" } else { "python3" };
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule generate\n",
                "  command = @PYTHON@ -c \"from pathlib import Path; Path('$out').write_text('generated')\"\n",
                "rule compile\n",
                "  command = @PYTHON@ -c \"from pathlib import Path; Path('$out').write_text('object'); Path('${out}.d').write_text('${out}$: $header\\n')\"\n",
                "  depfile = ${out}.d\n  deps = gcc\n",
                "build generated.h: generate\n",
                "build default.o: compile default.c\n  header = source.h\n",
                "build unrelated.o: compile unrelated.c\n  header = generated.h\n",
                "default default.o\n",
            )
            .replace("@PYTHON@", python),
        )
        .unwrap();
        fs::write(temp.path().join("default.c"), "source\n").unwrap();
        fs::write(temp.path().join("unrelated.c"), "source\n").unwrap();
        fs::write(temp.path().join("source.h"), "header\n").unwrap();

        for targets in [&["generated.h"][..], &["default.o", "unrelated.o"][..]] {
            let built = run(executable, temp.path(), targets);
            assert!(
                built.status.success(),
                "{} stdout={} stderr={}",
                executable.display(),
                String::from_utf8_lossy(&built.stdout),
                String::from_utf8_lossy(&built.stderr)
            );
        }

        let default_scan = run(executable, temp.path(), &["-t", "missingdeps"]);
        assert!(default_scan.status.success(), "{}", executable.display());
        assert!(!String::from_utf8_lossy(&default_scan.stdout).contains("Missing dep:"));
        let result = String::from_utf8_lossy(&default_scan.stdout).replace('\r', "");
        if let Some(expected) = &expected_default {
            assert_eq!(&result, expected);
        } else {
            expected_default = Some(result);
        }

        let unrelated = run(
            executable,
            temp.path(),
            &["-t", "missingdeps", "unrelated.o"],
        );
        assert_eq!(unrelated.status.code(), Some(3), "{}", executable.display());
        assert!(String::from_utf8_lossy(&unrelated.stdout).contains("Missing dep:"));
    }
}

#[test]
fn missingdeps_scans_plain_depfiles_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule generate\n  command = echo generated\n",
            "build generated.h: generate\n",
            "build out: generate source\n  depfile = out.d\n",
        ),
    )
    .unwrap();
    fs::write(temp.path().join("out.d"), "out: generated.h\n").unwrap();

    let arguments = ["-t", "missingdeps", "out"];
    let expected = run(ninja, temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout).replace('\r', ""),
        String::from_utf8_lossy(&expected.stdout).replace('\r', "")
    );
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn upstream_missing_dependency_scanner_corpus_matches_ninja_alias() {
    type MissingDepsCase = (
        &'static str,
        String,
        &'static [(&'static str, &'static str)],
    );

    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let base = concat!(
        "rule generator_rule\n  command = unused\n  deps = gcc\n",
        "rule compile_rule\n  command = unused\n  deps = gcc\n",
        "build generated_header: generator_rule\n",
        "build compiled_object: compile_rule\n",
    );
    let cases: &[MissingDepsCase] = &[
        ("EmptyGraph", "\n".to_owned(), &[]),
        ("NoMissingDep", base.to_owned(), &[]),
        (
            "MissingDepPresent",
            base.to_owned(),
            &[("compiled_object", "generated_header")],
        ),
        (
            "MissingDepFixedDirect",
            base.replace(
                "build compiled_object: compile_rule\n",
                "build compiled_object: compile_rule generated_header\n",
            ),
            &[("compiled_object", "generated_header")],
        ),
        (
            "MissingDepFixedIndirect",
            base.replace(
                "build compiled_object: compile_rule\n",
                concat!(
                    "build intermediate: generator_rule generated_header\n",
                    "build compiled_object: compile_rule intermediate\n",
                ),
            ),
            &[("compiled_object", "generated_header")],
        ),
        (
            "CyclicMissingDep",
            base.to_owned(),
            &[
                ("generated_header", "compiled_object"),
                ("compiled_object", "generated_header"),
            ],
        ),
        (
            "CycleInGraph",
            base.replace(
                "build generated_header: generator_rule\nbuild compiled_object: compile_rule\n",
                concat!(
                    "build generated_header: generator_rule compiled_object\n",
                    "build compiled_object: compile_rule generated_header\n",
                ),
            ),
            &[],
        ),
    ];

    for (name, manifest, dependencies) in cases {
        let mut observed = Vec::new();
        for candidate in [Path::new(&ninja), knight] {
            let temp = tempdir().unwrap();
            fs::write(temp.path().join("build.ninja"), manifest).unwrap();
            if !dependencies.is_empty() {
                let path = temp.path().join(".ninja_deps");
                let mut log = knight_build::deps_log::DepsLog::load(path).unwrap();
                for (output, input) in *dependencies {
                    log.record(output, 0, &[(*input).to_owned()]).unwrap();
                }
            }
            let alias = temp
                .path()
                .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
            let executable = if candidate == knight {
                install_ninja_alias(knight, &alias);
                alias.as_path()
            } else {
                candidate
            };
            let output = run(executable, temp.path(), &["-t", "missingdeps"]);
            observed.push((output.status.code(), output.stdout, output.stderr));
        }
        assert_eq!(observed[1], observed[0], "{name}");
    }
}

#[cfg(windows)]
#[test]
fn commands_and_compdb_options_match_ninja() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n  command = cc @$rspfile\n  rspfile = args.rsp\n",
            "  rspfile_content = $in\n",
            "rule link\n  command = link $in -o $out\n",
            "rule unused\n  command = echo unused\n",
            "rule validate\n  command = validate $in\n",
            "build a.o: cc a.c common.h\n",
            "build app: link a.o\n",
            "build unrelated: unused\n",
            "build validation: validate lint.in\n",
            "build checked: phony app |@ validation\n",
            "default checked\n",
        ),
    )
    .unwrap();

    let text_cases: &[&[&str]] = &[
        &["-t", "commands"],
        &["-t", "commands", "-s", "app"],
        &["-t", "commands", "-s", "./app"],
    ];
    for arguments in text_cases {
        let actual = run(knight, temp.path(), arguments);
        assert!(actual.status.success());
        assert!(!String::from_utf8_lossy(&actual.stdout).contains("unused"));
        if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
            let expected = run(Path::new(&ninja), temp.path(), arguments);
            assert_eq!(actual.status.code(), expected.status.code());
            assert_eq!(actual.stdout, expected.stdout, "arguments={arguments:?}");
            assert_eq!(actual.stderr, expected.stderr, "arguments={arguments:?}");
        }
    }

    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        for arguments in [
            &["-t", "rules", "-d"][..],
            &["-t", "targets", "all"][..],
            &["-t", "rules", "-x"][..],
            &["-t", "commands", "-x"][..],
            &["-t", "clean", "-x"][..],
        ] {
            let expected = run(Path::new(&ninja), temp.path(), arguments);
            let actual = run(knight, temp.path(), arguments);
            assert_eq!(actual.status.code(), expected.status.code());
            assert_eq!(actual.stdout, expected.stdout, "arguments={arguments:?}");
            assert_eq!(actual.stderr, expected.stderr, "arguments={arguments:?}");
        }
    }

    for arguments in [
        &["-t", "compdb"][..],
        &["-t", "compdb", "cc"][..],
        &["-t", "compdb", "-x", "cc"][..],
        &["-t", "compdb-targets", "checked"][..],
    ] {
        let actual = run(knight, temp.path(), arguments);
        assert!(actual.status.success(), "arguments={arguments:?}");
        let actual_json: serde_json::Value = serde_json::from_slice(&actual.stdout).unwrap();
        if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
            let expected = run(Path::new(&ninja), temp.path(), arguments);
            let expected_json: serde_json::Value =
                serde_json::from_slice(&expected.stdout).unwrap();
            assert_eq!(actual_json, expected_json, "arguments={arguments:?}");
        }
    }

    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        for arguments in [
            &["-t", "inputs", "checked", "-0Ed"][..],
            &["-t", "multi-inputs", "checked", "-0d,"][..],
            &["-t", "multi-inputs", "--delimiter", "::", "checked"][..],
        ] {
            let expected = run(Path::new(&ninja), temp.path(), arguments);
            let actual = run(knight, temp.path(), arguments);
            assert_eq!(actual.status.code(), expected.status.code());
            assert_eq!(actual.stdout, expected.stdout, "arguments={arguments:?}");
            assert_eq!(actual.stderr, expected.stderr, "arguments={arguments:?}");
        }
    }

    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        for target in ["checked", "validation"] {
            let arguments = ["-t", "query", target];
            let actual = run(knight, temp.path(), &arguments);
            let expected = run(Path::new(&ninja), temp.path(), &arguments);
            assert_eq!(actual.status.code(), expected.status.code());
            assert_eq!(actual.stdout, expected.stdout, "target={target}");
            assert_eq!(actual.stderr, expected.stderr, "target={target}");
        }
    }
}

#[test]
fn compdb_uses_ninjas_exact_json_control_escapes() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "rule cc\n  command = echo back\u{0008}space form\u{000c}feed\nbuild out: cc in\n",
    )
    .unwrap();
    let arguments = ["-t", "compdb"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert!(actual.stdout.windows(2).any(|bytes| bytes == b"\\b"));
    assert!(actual.stdout.windows(2).any(|bytes| bytes == b"\\f"));
}

#[test]
fn compdb_rsp_expansion_preserves_ninjas_first_marker_semantics() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n",
            "  command = echo -f unrelated && cc -f $rspfile\n",
            "  rspfile = args.rsp\n",
            "  rspfile_content = expanded content\n",
            "build out: cc in\n",
        ),
    )
    .unwrap();
    let arguments = ["-t", "compdb", "-x"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert!(String::from_utf8_lossy(&actual.stdout).contains("cc -f args.rsp"));
}

#[test]
fn nested_include_paths_resolve_from_the_working_directory_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    for nested_directive in ["include child.ninja", "subninja child.ninja"] {
        let temp = tempdir().unwrap();
        fs::create_dir(temp.path().join("dir")).unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            "include dir/parent.ninja\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("dir/parent.ninja"),
            format!("{nested_directive}\n"),
        )
        .unwrap();
        fs::write(
            temp.path().join("child.ninja"),
            "build working-directory-child: phony\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("dir/child.ninja"),
            "build containing-directory-child: phony\n",
        )
        .unwrap();

        let arguments = ["-t", "targets", "all"];
        let expected = run(ninja, temp.path(), &arguments);
        let actual = run(knight, temp.path(), &arguments);
        assert!(expected.status.success() && actual.status.success());
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>(),
            "directive={nested_directive}"
        );
    }
}

#[test]
fn phony_self_reference_policy_matches_ninja_tools() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("build.ninja"), "build a: phony a\n").unwrap();

    for policy in ["phonycycle=warn", "phonycycle=err"] {
        let arguments = ["-w", policy, "-t", "query", "a"];
        let actual = run(knight, temp.path(), &arguments);
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>(),
            "policy={policy}"
        );

        let actual_error = String::from_utf8_lossy(&actual.stderr);
        let expected_error = String::from_utf8_lossy(&expected.stderr);
        assert_eq!(
            actual_error
                .strip_prefix("knight: ")
                .unwrap_or(&actual_error)
                .trim(),
            expected_error
                .strip_prefix("ninja: ")
                .unwrap_or(&expected_error)
                .trim(),
            "policy={policy}"
        );
    }
}

#[test]
fn multi_output_edges_retain_real_self_cycles_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let cases = [
        "build a b: phony a\ndefault b\n",
        "build b a: phony a\ndefault b\n",
        "build a b: phony c\nbuild c: phony a\ndefault b\n",
        concat!(
            "build d: phony c\n",
            "build c: phony b\n",
            "build b: phony a\n",
            "build a e: phony d\n",
            "build f: phony e\n",
            "default f\n",
        ),
    ];

    for manifest in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let expected = run(ninja, temp.path(), &["-n"]);
        let actual = run(knight, temp.path(), &["-n"]);
        assert_eq!(actual.status.code(), expected.status.code(), "{manifest}");
        assert_eq!(actual.stdout, expected.stdout, "{manifest}");
        let expected_error = String::from_utf8_lossy(&expected.stderr).replace("ninja:", "tool:");
        let actual_error = String::from_utf8_lossy(&actual.stderr).replace("knight:", "tool:");
        assert_eq!(actual_error, expected_error, "{manifest}");
    }
}

#[test]
fn declaration_order_and_default_lookup_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("already-there"), "existing").unwrap();

    let cases = [
        (
            "forward rule",
            "build out: later\nrule later\n  command = echo\n",
        ),
        (
            "forward rule pool",
            "rule r\n  command = echo\n  pool = later\nbuild out: r\npool later\n  depth = 1\n",
        ),
        (
            "forward edge pool",
            "rule r\n  command = echo\nbuild out: r\n  pool = later\npool later\n  depth = 1\n",
        ),
        ("forward default", "default out\nbuild out: phony\n"),
        ("unknown existing default", "default already-there\n"),
        (
            "previously mentioned input default",
            "build out: phony source\ndefault source\n",
        ),
        (
            "duplicate rule binding uses last value",
            "rule r\n  command = first\n  command = second\nbuild out: r\n",
        ),
        (
            "zero-depth pool parses",
            "pool stopped\n  depth = 0\nrule r\n  command = echo\n  pool = stopped\nbuild out: r\n",
        ),
        (
            "empty response path mismatches content",
            "rule r\n  command = echo\n  rspfile =\n  rspfile_content = content\nbuild out: r\n",
        ),
    ];

    for (name, manifest) in cases {
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let expected = run(ninja, temp.path(), &["-t", "rules"]);
        let actual = run(knight, temp.path(), &["-t", "rules"]);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "case={name} actual={} expected={}",
            String::from_utf8_lossy(&actual.stderr),
            String::from_utf8_lossy(&expected.stderr)
        );
    }
}

#[test]
fn upstream_default_node_selection_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let cases = [
        (
            "all roots",
            concat!(
                "rule cat\n  command = echo $out\n",
                "build a: cat\n",
                "build b: cat\n",
                "build c: cat\n",
                "build d: cat\n",
            ),
        ),
        (
            "root cycle",
            "rule cat\n  command = echo $out\nbuild a: cat a\n",
        ),
        (
            "explicit defaults",
            concat!(
                "rule cat\n  command = echo $out\n",
                "build a: cat\n",
                "build b: cat\n",
                "build c: cat\n",
                "build d: cat\n",
                "third = c\n",
                "default a b\n",
                "default $third\n",
            ),
        ),
    ];

    for (name, manifest) in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        install_ninja_alias(knight, &alias);
        let expected = run(ninja, temp.path(), &["-n"]);
        let actual = run(&alias, temp.path(), &["-n"]);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }
}

#[test]
fn upstream_manifest_parser_acceptance_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let cases = [
        ("empty", ""),
        (
            "indented-comments",
            "  # comment\nrule cat\n  command = cat $in > $out\n  # generator = 1\n  restat = 1 # literal\nbuild result: cat input\n  # edge comment\n",
        ),
        (
            "indented-blank-lines",
            "  \nrule cat\n  command = cat $in > $out\n  \nbuild result: cat input\n  \nvariable=1\n",
        ),
        (
            "response-file-override",
            "rule cat\n  command = cat $rspfile > $out\n  rspfile = $rspfile\n  rspfile_content = $in\nbuild out: cat in\n  rspfile = out.rsp\n",
        ),
        (
            "continuations",
            "rule link\n  command = foo bar $\n    baz\nbuild a: link c $\n d e f\n",
        ),
        ("literal-backslashes", "foo = bar\\baz\nfoo2 = bar\\ baz\n"),
        ("hash-in-value", "foo = not # a comment\n"),
        (
            "escaped-paths",
            "rule spaces\n  command = something\nbuild foo$ bar: spaces $$one two$$$ three\n",
        ),
        (
            "reserved-words",
            "rule build\n  command = rule run $out\nbuild subninja: build include default foo.cc\ndefault subninja\n",
        ),
        (
            "empty-implicit-output-list",
            "rule cat\n  command = cat $in > $out\nbuild foo | : cat bar\n",
        ),
        (
            "implicit-output-only",
            "rule cat\n  command = cat $in > $out\nbuild | imp: cat bar\n",
        ),
        (
            "multiple-outputs-with-deps",
            "rule cc\n  command = cc\n  deps = gcc\n  depfile = deps.d\nbuild a.o b.o: cc c.cc\n",
        ),
        (
            "all-dependency-separators",
            "rule cat\n  command = cat\nbuild explicit | implicit: cat in | implicit-in || order |@ validation\n",
        ),
        (
            "zero-depth-pool",
            "pool held\n  depth = 0\nrule cat\n  command = cat\n  pool = held\nbuild out: cat in\n",
        ),
        (
            "crlf",
            "pool link_pool\r\n  depth = 15\r\n\r\nrule xyz\r\n  command = something$expand \r\n  description = YAY!\r\n",
        ),
        (
            "dyndep-order-only-input",
            "rule cat\n  command = cat $in > $out\nbuild result: cat in || dd\n  dyndep = dd\n",
        ),
        (
            "dyndep-from-rule",
            "rule cat\n  command = cat $in > $out\n  dyndep = $in\nbuild result: cat in\n",
        ),
        (
            "dashed-variable-names",
            "ninja_required_version = 1.14\nfoo-bar = value\nrule echo\n  command = echo $foo-bar\nbuild out: echo\n",
        ),
    ];

    for (name, manifest) in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let arguments = ["-t", "targets", "all"];
        let expected = run(ninja, temp.path(), &arguments);
        let actual = run(knight, temp.path(), &arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "case={name} manifest={manifest:?} ninja_stdout={} ninja_stderr={} knight_stdout={} knight_stderr={}",
            String::from_utf8_lossy(&expected.stdout),
            String::from_utf8_lossy(&expected.stderr),
            String::from_utf8_lossy(&actual.stdout),
            String::from_utf8_lossy(&actual.stderr),
        );
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>(),
            "case={name}"
        );
    }
}

#[test]
fn upstream_manifest_command_expansion_cases_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let cases = [
        (
            "in-newline",
            "rule cat\n  command = cat $in_newline > $out\nbuild out: cat in in2\n",
        ),
        (
            "variables",
            concat!(
                "l = one-letter-test\n",
                "rule link\n  command = ld $l $extra $with_under -o $out $in\n",
                "extra = -pthread\n",
                "with_under = -under\n",
                "build a: link b c\n",
                "nested1 = 1\n",
                "nested2 = $nested1/2\n",
                "build supernested: link x\n  extra = $nested2/3\n",
            ),
        ),
        (
            "variable-scope",
            concat!(
                "foo = bar\n",
                "rule cmd\n  command = cmd $foo $in $out\n",
                "build inner: cmd a\n  foo = baz\n",
                "build outer: cmd b\n",
            ),
        ),
        (
            "dollars",
            concat!(
                "rule foo\n",
                "  command = ${out}bar$$baz$$$\n",
                "blah\n",
                "x = $$dollar\n",
                "build $x: foo y\n",
            ),
        ),
        (
            "continuation",
            "rule link\n  command = foo bar $\n    baz\nbuild a: link c $\n d e f\n",
        ),
    ];

    for (name, manifest) in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let expected = run(ninja, temp.path(), &["-t", "commands"]);
        let actual = run(knight, temp.path(), &["-t", "commands"]);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }
}

#[test]
fn upstream_manifest_parser_rejection_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let cases = [
        ("bare-identifier", "foobar\n"),
        ("bad-assignment", "x 3\n"),
        ("bad-dollar", "x = $\n"),
        ("build-without-path", "build\n"),
        ("unknown-rule", "build x: unknown z\n"),
        ("double-colon", "build x:: unknown z\n"),
        ("missing-command", "rule cat\n"),
        (
            "duplicate-rule",
            "rule cat\n  command = echo\nrule cat\n  command = echo\n",
        ),
        (
            "response-file-pair",
            "rule cat\n  command = echo\n  rspfile = cat.rsp\n",
        ),
        (
            "unterminated-variable",
            "rule cat\n  command = ${broken\nfoo = bar\n",
        ),
        (
            "bad-path-escape",
            "rule cat\n  command = cat\nbuild $.: cat foo\n",
        ),
        ("bad-rule-name", "rule %foo\n"),
        (
            "unknown-rule-binding",
            "rule cc\n  command = foo\n  othervar = bar\n",
        ),
        (
            "bad-indented-binding",
            "rule cc\n  command = foo\n  && bar\n",
        ),
        (
            "tab-indentation",
            "rule cc\n\tcommand = echo\nbuild out: cc\n",
        ),
        ("default-without-target", "default\n"),
        ("unknown-default", "default nonexistent\n"),
        (
            "junk-after-default",
            "rule r\n  command = r\nbuild b: r\ndefault b:\n",
        ),
        ("empty-default-path", "default $a\n"),
        (
            "empty-build-path",
            "rule r\n  command = r\nbuild $a: r $c\n",
        ),
        (
            "indent-after-blank",
            "rule r\n  command = r\n  \n  generator = 1\n",
        ),
        ("pool-without-name", "pool\n"),
        ("pool-without-depth", "pool foo\n"),
        (
            "duplicate-pool",
            "pool foo\n  depth = 4\npool foo\n  depth = 2\n",
        ),
        ("negative-pool", "pool foo\n  depth = -1\n"),
        ("nonnumeric-pool", "pool foo\n  depth = foo\n"),
        ("unknown-pool-binding", "pool foo\n  bar = 1\n"),
        (
            "unknown-pool-reference",
            "rule run\n  command = echo\n  pool = absent\nbuild out: run in\n",
        ),
        (
            "duplicate-implicit-output",
            "rule cat\n  command = cat\nbuild foo baz | foo baq foo: cat bar\n",
        ),
        (
            "dyndep-not-input",
            "rule touch\n  command = touch $out\nbuild result: touch\n  dyndep = notin\n",
        ),
    ];

    for (name, manifest) in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let arguments = ["-t", "targets", "all"];
        let expected = run(ninja, temp.path(), &arguments);
        let actual = run(knight, temp.path(), &arguments);
        assert_eq!(
            expected.status.code(),
            Some(1),
            "Ninja corpus premise changed for {name}: stdout={} stderr={}",
            String::from_utf8_lossy(&expected.stdout),
            String::from_utf8_lossy(&expected.stderr),
        );
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "case={name} manifest={manifest:?} ninja_stdout={} ninja_stderr={} knight_stdout={} knight_stderr={}",
            String::from_utf8_lossy(&expected.stdout),
            String::from_utf8_lossy(&expected.stderr),
            String::from_utf8_lossy(&actual.stdout),
            String::from_utf8_lossy(&actual.stderr),
        );
    }
}

#[test]
fn manifest_diagnostics_match_ninja_when_invoked_as_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let cases = [
        ("bare identifier at EOF", "foobar"),
        ("bare identifier", "foobar\n"),
        ("short identifier at EOF", "subn"),
        ("bad assignment", "x 3\n"),
        ("bad assignment at EOF", "x 3"),
        ("assignment at EOF", "x = 3"),
        ("bad dollar at EOF", "x = $"),
        ("continued at EOF", "x = $\n"),
        ("bad continued escape", "x = $\n $[\n"),
        ("bad continued CRLF escape", "x = $\r\n $[\r\n"),
        ("multiple continuations at EOF", "x = a$\n b$\n $\n"),
        ("build without path", "build\n"),
        ("unknown rule", "build x: unknown z\n"),
        ("unknown rule with CRLF", "build x: unknown z\r\n"),
        ("double colon", "build x:: unknown z\n"),
        (
            "continued colon",
            "rule cat\n  command = cat ok\nbuild x: cat $\n :\n",
        ),
        ("missing command", "rule cat\n"),
        (
            "duplicate rule",
            "rule cat\n  command = echo\nrule cat\n  command = echo\n",
        ),
        (
            "response-file pair",
            "rule cat\n  command = echo\n  rspfile = cat.rsp\n",
        ),
        ("bad rule name", "rule %foo\n"),
        (
            "unterminated variable",
            "rule cat\n  command = ${broken\nfoo = bar\n",
        ),
        (
            "bad path escape",
            "rule cat\n  command = cat\nbuild $.: cat foo\n",
        ),
        (
            "escaped output colon",
            "rule cat\n  command = cat\nbuild $: cat foo\n",
        ),
        (
            "unknown rule binding",
            "rule cc\n  command = foo\n  othervar = bar\n",
        ),
        (
            "bad indented binding",
            "rule cc\n  command = foo\n  && bar\n",
        ),
        (
            "tab indentation",
            "rule cc\n\tcommand = echo\nbuild out: cc\n",
        ),
        ("default without target", "default\n"),
        ("unknown default", "default nonexistent\n"),
        (
            "junk after default",
            "rule r\n  command = r\nbuild b: r\ndefault b:\n",
        ),
        ("empty default path", "default $a\n"),
        (
            "empty build path",
            "rule r\n  command = r\nbuild $a: r $c\n",
        ),
        (
            "indent after blank",
            "rule r\n  command = r\n  \n  generator = 1\n",
        ),
        ("pool without name", "pool\n"),
        ("pool without depth", "pool foo\n"),
        (
            "duplicate pool",
            "pool foo\n  depth = 4\npool foo\n  depth = 2\n",
        ),
        ("negative pool", "pool foo\n  depth = -1\n"),
        ("nonnumeric pool", "pool foo\n  depth = foo\n"),
        ("unknown pool binding", "pool foo\n  bar = 1\n"),
        (
            "unknown pool reference",
            "rule run\n  command = echo\n  pool = absent\nbuild out: run in\n",
        ),
        (
            "duplicate implicit output",
            "rule cat\n  command = cat\nbuild foo baz | foo baq foo: cat bar\n",
        ),
        (
            "repeated duplicate implicit outputs",
            "rule cat\n  command = cat\nbuild foo foo foo | foo foo foo foo: cat bar\n",
        ),
        (
            "duplicate edge with multiple outputs",
            concat!(
                "rule cat\n",
                "  command = cat $in > $out\n",
                "build out1 out2: cat in1\n",
                "build out1: cat in2\n",
                "build final: cat out1\n",
            ),
        ),
        (
            "dyndep not input",
            "rule touch\n  command = touch $out\nbuild result: touch\n  dyndep = notin\n",
        ),
        (
            "order-only output separator",
            "rule r\n  command = r\nbuild x || y: r\n",
        ),
        (
            "implicit after order-only input",
            "rule r\n  command = r\nbuild x: r y || z | q\n",
        ),
        (
            "repeated implicit separator",
            "rule r\n  command = r\nbuild x: r y | z | q\n",
        ),
        (
            "repeated order-only separator",
            "rule r\n  command = r\nbuild x: r y || z || q\n",
        ),
        (
            "order-only after validation",
            "rule r\n  command = r\nbuild x: r y |@ z || q\n",
        ),
        (
            "colon in input list",
            "rule r\n  command = r\nbuild x: r y : q\n",
        ),
    ];

    for (name, manifest) in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        install_ninja_alias(knight, &alias);
        let arguments = ["-t", "targets", "all"];
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        let actual = run(&alias, temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }
}

#[test]
fn included_manifest_diagnostics_match_ninja_without_sacrificing_cycle_detection() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let cases = [
        ("missing include", "include absent.ninja\n", None),
        ("missing subninja", "subninja absent.ninja\n", None),
        ("missing CRLF include", "include absent.ninja\r\n", None),
        ("missing empty include", "include\n", None),
        ("missing empty subninja", "subninja\n", None),
        ("empty include path", "include $missing\n", None),
        ("empty subninja path", "subninja $missing\n", None),
        ("broken include", "include child.ninja\n", Some("build\n")),
        ("broken subninja", "subninja child.ninja\n", Some("build\n")),
        (
            "duplicate edge in included file",
            "subninja child.ninja\n",
            Some(concat!(
                "rule cat\n",
                "  command = cat $in > $out\n",
                "build out1 out2: cat in1\n",
                "build out1: cat in2\n",
                "build final: cat out1\n",
            )),
        ),
    ];

    for (name, manifest, child) in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        if let Some(child) = child {
            fs::write(temp.path().join("child.ninja"), child).unwrap();
        }
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        install_ninja_alias(knight, &alias);
        let arguments = ["-t", "targets", "all"];
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        let actual = run(&alias, temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }

    for (name, arguments, directory_manifest) in [
        ("missing default manifest", vec![], false),
        (
            "missing explicit manifest",
            vec!["-f", "absent.ninja"],
            false,
        ),
        ("empty manifest path", vec!["-f", ""], false),
        ("directory manifest", vec!["-f", "folder"], true),
    ] {
        let temp = tempdir().unwrap();
        if directory_manifest {
            fs::create_dir(temp.path().join("folder")).unwrap();
        }
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        install_ninja_alias(knight, &alias);
        let mut arguments = arguments;
        arguments.extend(["-t", "targets", "all"]);
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        let actual = run(&alias, temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }

    let temp = tempdir().unwrap();
    fs::write(temp.path().join("build.ninja"), "include child.ninja\n").unwrap();
    fs::write(temp.path().join("child.ninja"), "include build.ninja\n").unwrap();
    let cycle = run(knight, temp.path(), &["-t", "targets", "all"]);
    assert_eq!(cycle.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&cycle.stderr).contains("include cycle detected"),
        "stderr={}",
        String::from_utf8_lossy(&cycle.stderr)
    );
}

#[test]
fn required_version_uses_ninja_major_minor_compatibility() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    for version in ["1.14", "1.14.1", "1.14.99", "1.15", "2.0", "garbage"] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!("ninja_required_version = {version}\nbuild all: phony\n"),
        )
        .unwrap();
        let arguments = ["-t", "targets", "all"];
        let expected = run(ninja, temp.path(), &arguments);
        let actual = run(knight, temp.path(), &arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "version={version} ninja_stdout={} ninja_stderr={} knight_stdout={} knight_stderr={}",
            String::from_utf8_lossy(&expected.stdout),
            String::from_utf8_lossy(&expected.stderr),
            String::from_utf8_lossy(&actual.stdout),
            String::from_utf8_lossy(&actual.stderr),
        );
    }
}

#[test]
fn required_version_warnings_and_newline_escape_gating_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    for (version, warns) in [("0.99", true), ("garbage", true), ("1.14.1", false)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!("ninja_required_version = {version}\nbuild all: phony\n"),
        )
        .unwrap();
        for executable in [ninja, knight] {
            let result = run(executable, temp.path(), &["-t", "targets", "all"]);
            assert!(result.status.success(), "version={version}");
            assert_eq!(
                String::from_utf8_lossy(&result.stderr).contains("warning:"),
                warns,
                "version={version} executable={}",
                executable.display()
            );
        }
    }

    for version in ["0.99", "garbage", "1.13", "1.14", "1.14.99", "2.0"] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                "ninja_required_version = {version}\nvalue = before$^after\nbuild all: phony\n"
            ),
        )
        .unwrap();
        let expected = run(ninja, temp.path(), &["-t", "targets", "all"]);
        let actual = run(knight, temp.path(), &["-t", "targets", "all"]);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "version={version} ninja_stderr={} knight_stderr={}",
            String::from_utf8_lossy(&expected.stderr),
            String::from_utf8_lossy(&actual.stderr)
        );
    }
}

#[test]
fn edge_bindings_can_name_edge_paths_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule echo\n",
            "  command = echo\n",
            "build $output | $implicit_output: echo $input | $implicit_input || $order_only |@ $validation\n",
            "  output = explicit.out\n",
            "  implicit_output = implicit.out\n",
            "  input = explicit.in\n",
            "  implicit_input = implicit.in\n",
            "  order_only = order.in\n",
            "  validation = validation.in\n",
            "default explicit.out\n",
        ),
    )
    .unwrap();
    let arguments = ["-t", "query", "explicit.out"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    assert!(
        expected.status.success(),
        "reference rejected edge path bindings: {}",
        String::from_utf8_lossy(&expected.stderr)
    );
    let actual = run(knight, temp.path(), &arguments);
    assert!(
        actual.status.success(),
        "Knight rejected edge path bindings: {}",
        String::from_utf8_lossy(&actual.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&expected.stdout)
            .lines()
            .collect::<Vec<_>>()
    );
}

#[test]
fn dyndep_parser_and_lexer_corpus_matches_ninja_byte_for_byte() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    let alias = temp
        .path()
        .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
    install_ninja_alias(knight, &alias);
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule touch\n  command = echo built > $out\n",
            "build out otherout: touch || deps.dd\n  dyndep = deps.dd\n",
            "build out2: touch || deps.dd\n  dyndep = deps.dd\n",
            "default out out2\n",
        ),
    )
    .unwrap();

    let cases = [
        ("empty", ""),
        ("version unexpected EOF", "ninja_dyndep_version = 1.0"),
        ("unsupported version 0", "ninja_dyndep_version = 0\n"),
        ("unsupported version 1.1", "ninja_dyndep_version = 1.1\n"),
        (
            "duplicate version",
            "ninja_dyndep_version = 1\nninja_dyndep_version = 1\n",
        ),
        (
            "missing version other variable",
            "not_ninja_dyndep_version = 1\n",
        ),
        ("missing version build", "build out: dyndep\n"),
        ("unexpected equals", "= 1\n"),
        ("unexpected indent", " = 1\n"),
        (
            "duplicate output",
            "ninja_dyndep_version = 1\nbuild out: dyndep\nbuild out: dyndep\n",
        ),
        (
            "duplicate edge through other output",
            "ninja_dyndep_version = 1\nbuild out: dyndep\nbuild otherout: dyndep\n",
        ),
        ("build EOF", "ninja_dyndep_version = 1\nbuild"),
        ("missing output", "ninja_dyndep_version = 1\nbuild :\n"),
        (
            "output without build statement",
            "ninja_dyndep_version = 1\nbuild missing: dyndep\n",
        ),
        ("output EOF", "ninja_dyndep_version = 1\nbuild out"),
        ("missing rule", "ninja_dyndep_version = 1\nbuild out:"),
        ("wrong rule", "ninja_dyndep_version = 1\nbuild out: touch"),
        (
            "statement EOF",
            "ninja_dyndep_version = 1\nbuild out: dyndep",
        ),
        (
            "explicit output",
            "ninja_dyndep_version = 1\nbuild out exp: dyndep\n",
        ),
        (
            "explicit input",
            "ninja_dyndep_version = 1\nbuild out: dyndep exp\n",
        ),
        (
            "order-only input",
            "ninja_dyndep_version = 1\nbuild out: dyndep ||\n",
        ),
        (
            "validation input",
            "ninja_dyndep_version = 1\nbuild out: dyndep |@ validation\n",
        ),
        (
            "wrong binding",
            "ninja_dyndep_version = 1\nbuild out: dyndep\n  not_restat = 1\n",
        ),
        (
            "second binding",
            "ninja_dyndep_version = 1\nbuild out: dyndep\n  restat = 1\n  restat = 1\n",
        ),
        (
            "continued version",
            "ninja_dyndep_version = 1$\n .0\nbuild out: dyndep\n",
        ),
        (
            "version continued through build line",
            "ninja_dyndep_version = 1$\nbuild out: dyndep\n",
        ),
        (
            "unsupported newline escape",
            "ninja_dyndep_version = 1\nbuild out$^x: dyndep\n",
        ),
        (
            "escaped output colon",
            "ninja_dyndep_version = 1\nbuild out$: dyndep\n",
        ),
        (
            "bad braced escape",
            "ninja_dyndep_version = 1\nbuild out${bad: dyndep\n",
        ),
        (
            "continued build path",
            "ninja_dyndep_version = 1\nbuild $\n out: dyndep\n",
        ),
        ("comment at EOF", "ninja_dyndep_version = 1\n# comment"),
    ];
    for (name, dyndep) in cases {
        fs::write(temp.path().join("deps.dd"), dyndep).unwrap();
        let arguments = ["-n"];
        let expected = run(ninja, temp.path(), &arguments);
        let actual = run(&alias, temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }
}

#[test]
fn dyndep_output_conflict_diagnostic_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule touch\n  command = echo built > $out\n",
            "build out1: touch || dd1\n  dyndep = dd1\n",
            "build out2: touch || dd2\n  dyndep = dd2\n",
            "default out1 out2\n",
        ),
    )
    .unwrap();
    fs::write(
        temp.path().join("dd1"),
        "ninja_dyndep_version = 1\nbuild out1 | shared: dyndep\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("dd2"),
        "ninja_dyndep_version = 1\nbuild out2 | shared: dyndep\n",
    )
    .unwrap();

    let expected = run(Path::new(&ninja), temp.path(), &["-n"]);
    let actual = run(knight, temp.path(), &["-n"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    let expected_error = String::from_utf8_lossy(&expected.stderr)
        .replace("ninja:", "tool:")
        .replace("ninja.exe:", "tool:");
    let actual_error = String::from_utf8_lossy(&actual.stderr)
        .replace("knight:", "tool:")
        .replace("knight.exe:", "tool:");
    assert_eq!(actual_error, expected_error);
}

#[test]
fn dyndep_file_entry_ownership_diagnostics_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let cases = [
        (
            "missing entry",
            concat!(
                "rule touch\n  command = echo built > $out\n",
                "build out: touch || dd\n  dyndep = dd\n",
                "default out\n",
            ),
            "ninja_dyndep_version = 1\n",
        ),
        (
            "entry without binding",
            concat!(
                "rule touch\n  command = echo built > $out\n",
                "build out: touch || dd\n  dyndep = dd\n",
                "build extra: touch || dd\n",
                "default out\n",
            ),
            concat!(
                "ninja_dyndep_version = 1\n",
                "build out: dyndep\n",
                "build extra: dyndep\n",
            ),
        ),
        (
            "existing output repeated dynamically",
            concat!(
                "rule touch\n  command = echo built > $out\n",
                "build out | existing: touch || dd\n  dyndep = dd\n",
                "default out\n",
            ),
            concat!(
                "ninja_dyndep_version = 1\n",
                "build out | existing: dyndep\n",
            ),
        ),
    ];

    for (name, manifest, dyndep) in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("dd"), dyndep).unwrap();
        let expected = run(ninja, temp.path(), &["-n"]);
        let actual = run(knight, temp.path(), &["-n"]);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        let expected_error = String::from_utf8_lossy(&expected.stderr)
            .replace("ninja:", "tool:")
            .replace("ninja.exe:", "tool:");
        let actual_error = String::from_utf8_lossy(&actual.stderr)
            .replace("knight:", "tool:")
            .replace("knight.exe:", "tool:");
        assert_eq!(actual_error, expected_error, "case={name}");
    }
}

#[test]
fn missing_dyndep_diagnostic_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule touch\n  command = echo built > $out\n",
            "build out: touch || dd\n  dyndep = dd\n",
            "default out\n",
        ),
    )
    .unwrap();

    let expected = run(Path::new(&ninja), temp.path(), &["-n"]);
    let actual = run(knight, temp.path(), &["-n"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    let expected_error = String::from_utf8_lossy(&expected.stderr)
        .replace("ninja:", "tool:")
        .replace("ninja.exe:", "tool:");
    let actual_error = String::from_utf8_lossy(&actual.stderr)
        .replace("knight:", "tool:")
        .replace("knight.exe:", "tool:");
    assert_eq!(actual_error, expected_error);
}

#[test]
fn rootless_graph_error_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "rule r\n  command = echo\nbuild a: r a\n",
    )
    .unwrap();
    let expected = run(Path::new(&ninja), temp.path(), &[]);
    let actual = run(knight, temp.path(), &[]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(
        String::from_utf8_lossy(&actual.stderr)
            .strip_prefix("knight: error: ")
            .unwrap()
            .trim(),
        String::from_utf8_lossy(&expected.stderr)
            .strip_prefix("ninja: error: ")
            .unwrap()
            .trim()
    );
}

#[test]
fn recompact_does_not_create_missing_metadata_logs() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("build.ninja"), "build all: phony\n").unwrap();
    let actual = run(knight, temp.path(), &["-t", "recompact"]);
    assert!(actual.status.success());
    assert!(!temp.path().join(".ninja_log").exists());
    assert!(!temp.path().join(".ninja_deps").exists());
}

#[test]
fn deps_recompact_discards_entries_without_live_deps_edges_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = concat!(
        "rule tracked\n  command = unused\n  deps = gcc\n",
        "rule plain\n  command = unused\n",
        "build live.o: tracked\n",
        "build dead.o: plain\n",
    );
    let mut observed = Vec::new();
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let deps_path = temp.path().join(".ninja_deps");
        let mut log = knight_build::deps_log::DepsLog::load(deps_path.clone()).unwrap();
        log.record("live.o", 1, &["live.h".to_owned()]).unwrap();
        log.record("dead.o", 2, &["dead.h".to_owned()]).unwrap();
        drop(log);

        let output = run(executable, temp.path(), &["-t", "recompact"]);
        assert!(output.status.success(), "{}", executable.display());
        let compact = knight_build::deps_log::DepsLog::load(deps_path.clone()).unwrap();
        assert!(compact.get("live.o").is_some(), "{}", executable.display());
        assert!(compact.get("dead.o").is_none(), "{}", executable.display());
        observed.push((output.stdout, output.stderr, fs::read(deps_path).unwrap()));
    }
    assert_eq!(observed[1], observed[0]);
}

#[test]
fn deps_log_bad_tail_recovery_matches_ninja_alias() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let manifest = "rule cc\n  command = unused\n  deps = gcc\nbuild out.o: cc\n";
    let mut observed = Vec::new();
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let deps_path = temp.path().join(".ninja_deps");
        let mut log = knight_build::deps_log::DepsLog::load(deps_path.clone()).unwrap();
        log.record("out.o", 1, &["foo.h".to_owned(), "bar.h".to_owned()])
            .unwrap();
        drop(log);
        let valid_length = fs::metadata(&deps_path).unwrap().len();
        let mut contents = fs::read(&deps_path).unwrap();
        contents.extend_from_slice(&[
            0x0c, 0x00, 0x00, 0x00, b'f', b'o', b'o', b'.', b'h', 0x00, 0x00, 0x00, 0xfe, 0xff,
            0xff, 0xff,
        ]);
        fs::write(&deps_path, contents).unwrap();

        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        let executable = if executable == knight {
            install_ninja_alias(knight, &alias);
            alias.as_path()
        } else {
            executable
        };
        let output = run(executable, temp.path(), &["-t", "deps", "out.o"]);
        assert!(output.status.success(), "{}", executable.display());
        assert_eq!(fs::metadata(&deps_path).unwrap().len(), valid_length);
        observed.push((output.stdout, output.stderr, fs::read(deps_path).unwrap()));
    }
    assert_eq!(observed[1], observed[0]);
}

#[test]
fn recompact_discards_incompatible_metadata_without_recreating_it() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    for filename in [".ninja_log", ".ninja_deps"] {
        let mut statuses = Vec::new();
        for executable in [ninja, knight] {
            let temp = tempdir().unwrap();
            fs::write(temp.path().join("build.ninja"), "build all: phony\n").unwrap();
            fs::write(temp.path().join(filename), "garbage\n").unwrap();
            let result = run(executable, temp.path(), &["-t", "recompact"]);
            statuses.push(result.status.code());
            assert!(
                !temp.path().join(filename).exists(),
                "filename={filename} executable={}",
                executable.display()
            );
        }
        assert_eq!(statuses[1], statuses[0], "filename={filename}");
    }
}

#[test]
fn metadata_log_creation_phases_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let cases: &[&[&str]] = &[
        &[],
        &["-n"],
        &["-t", "targets", "all"],
        &["-t", "query", "all"],
        &["-t", "recompact"],
    ];

    for arguments in cases {
        let mut observed = Vec::new();
        for executable in [ninja, knight] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                "builddir = metadata/sub\nbuild all: phony\ndefault all\n",
            )
            .unwrap();
            let result = run(executable, temp.path(), arguments);
            assert!(
                result.status.success(),
                "arguments={arguments:?} executable={} stdout={} stderr={}",
                executable.display(),
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr),
            );
            let builddir = temp.path().join("metadata/sub");
            observed.push((
                builddir.exists(),
                builddir.join(".ninja_log").exists(),
                builddir.join(".ninja_deps").exists(),
                builddir.join(".ninja_lock").exists(),
            ));
        }
        assert_eq!(observed[1], observed[0], "arguments={arguments:?}");
    }
}

#[test]
fn completed_builds_remove_stale_lock_files_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    for arguments in [&[][..], &["-n"][..]] {
        for executable in [ninja, knight] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                "builddir = metadata/sub\nbuild all: phony\ndefault all\n",
            )
            .unwrap();
            fs::create_dir_all(temp.path().join("metadata/sub")).unwrap();
            fs::write(temp.path().join("metadata/sub/.ninja_lock"), "stale").unwrap();
            let result = run(executable, temp.path(), arguments);
            assert!(
                result.status.success(),
                "executable={}",
                executable.display()
            );
            assert!(
                !temp.path().join("metadata/sub/.ninja_lock").exists(),
                "executable={} arguments={arguments:?}",
                executable.display()
            );
        }
    }
}

#[test]
fn restat_help_exit_status_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let arguments = ["-t", "restat", "--help"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&expected.stdout)
            .lines()
            .collect::<Vec<_>>()
    );
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn build_log_restat_uses_ninjas_c_numeric_prefix_parsing() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let mut observed = Vec::new();
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join(".ninja_log");
        fs::write(
            &log_path,
            "# ninja log v7\n123suffix\t-4end\t5x\tout with space\tcommand\n",
        )
        .unwrap();
        let output = run(executable, temp.path(), &["-t", "restat", "other"]);
        assert!(output.status.success(), "{}", executable.display());
        observed.push((output.stdout, output.stderr, fs::read(log_path).unwrap()));
    }
    assert_eq!(observed[1], observed[0]);
    assert_eq!(
        observed[1].2,
        b"# ninja log v7\n123\t-4\t5\tout with space\tc\n"
    );
}

#[test]
fn restat_discards_incompatible_build_logs_without_recreating_them() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join(".ninja_log"), "incompatible\n").unwrap();
        let output = run(executable, temp.path(), &["-t", "restat"]);
        assert!(output.status.success(), "{}", executable.display());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert!(!temp.path().join(".ninja_log").exists());
    }
}

#[test]
fn restat_compaction_missing_outputs_and_dry_run_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("existing"), "data").unwrap();
    let initial = concat!(
        "# ninja log v7\n",
        "0\t1\t111\texisting\taaa\n",
        "2\t3\t222\tmissing\tbbb\n",
        "4\t5\t333\texisting\tccc\n",
        "6\t7\t444\tuntouched\tddd\n",
    );
    let parse_log = |contents: &str| {
        let mut entries = contents
            .lines()
            .skip(1)
            .map(|line| {
                let fields = line.split('\t').collect::<Vec<_>>();
                (
                    fields[3].to_owned(),
                    fields[0].parse::<u32>().unwrap(),
                    fields[1].parse::<u32>().unwrap(),
                    fields[2].parse::<u64>().unwrap(),
                    u64::from_str_radix(fields[4], 16).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        entries.sort();
        entries
    };

    let mut observed = Vec::new();
    for executable in [ninja, knight] {
        fs::write(temp.path().join(".ninja_log"), initial).unwrap();
        let result = run(
            executable,
            temp.path(),
            &["-n", "-t", "restat", "existing", "missing"],
        );
        assert!(
            result.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let contents = fs::read_to_string(temp.path().join(".ninja_log")).unwrap();
        assert_ne!(contents, initial, "-n must still restat the log");
        observed.push(parse_log(&contents));
    }
    assert_eq!(observed[1], observed[0]);
    assert_eq!(observed[1].len(), 3, "duplicate records must compact");
    assert_eq!(
        observed[1]
            .iter()
            .find(|entry| entry.0 == "missing")
            .unwrap()
            .3,
        0
    );
    assert_eq!(
        observed[1]
            .iter()
            .find(|entry| entry.0 == "untouched")
            .unwrap()
            .3,
        444
    );
}

#[test]
fn top_level_help_uses_ninja_exit_status_and_stream() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let expected = run(Path::new(&ninja), temp.path(), &["--help"]);
    let actual = run(knight, temp.path(), &["--help"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert!(actual.stdout.is_empty());
    assert_eq!(actual.stdout.is_empty(), expected.stdout.is_empty());
    assert!(!actual.stderr.is_empty());
}

#[test]
fn manifest_tool_help_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("build.ninja"), "build all: phony\n").unwrap();
    for (tool, help) in [
        ("clean", "-h"),
        ("commands", "-h"),
        ("compdb", "-h"),
        ("compdb-targets", "-h"),
        ("inputs", "--help"),
        ("multi-inputs", "--help"),
        ("rules", "-h"),
    ] {
        let arguments = ["-t", tool, help];
        let expected = run(ninja, temp.path(), &arguments);
        let actual = run(knight, temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code(), "tool={tool}");
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>(),
            "tool={tool}"
        );
        assert_eq!(actual.stderr, expected.stderr, "tool={tool}");
    }
}

#[cfg(unix)]
#[test]
fn browse_help_matches_ninjas_embedded_tool() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("build.ninja"), "build all: phony\n").unwrap();
    let arguments = ["-t", "browse", "--help"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[cfg(unix)]
#[test]
fn browse_server_serves_query_pages_like_ninja() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let alias = temp.path().join("ninja");
    install_ninja_alias(knight, &alias);
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n  command = cc $in -o $out\n",
            "build out: cc source\n",
            "default out\n",
        ),
    )
    .unwrap();

    let fetch = |executable: &Path| {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut child = Command::new(executable)
            .current_dir(temp.path())
            .args([
                "-t",
                "browse",
                "--no-browser",
                "--hostname",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "out",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(stream) => break stream,
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("browse server did not start: {error}");
                }
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(b"GET /?out HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let separator = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        response[separator + 4..].to_vec()
    };

    let expected = fetch(Path::new(&ninja));
    let actual = fetch(&alias);
    assert_eq!(actual, expected);
    let page = String::from_utf8(actual).unwrap();
    assert!(page.contains("<h1><tt>out</tt></h1>"));
    assert!(page.contains("rule <tt>cc</tt>"));
    assert!(page.contains("?source"));
}

#[test]
fn tool_option_permutation_and_deps_target_errors_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n  command = echo compile $in -o $out\n",
            "rule link\n  command = echo link $in -o $out\n",
            "build obj: cc source\n",
            "build app: link obj\n",
            "build other: cc unique\n",
            "default app\n",
        ),
    )
    .unwrap();

    for arguments in [
        &["-t", "commands", "-x"][..],
        &["-t", "clean", "cc"][..],
        &["-t", "clean", "-r", "app"][..],
        &["-t", "clean", "-x"][..],
        &["-t", "inputs", "-x"][..],
        &["-t", "inputs", "--bogus"][..],
        &["-t", "inputs", "--definitely-invalid"][..],
        &["-t", "inputs", "--help=ignored"][..],
        &["-t", "inputs", "--print0=ignored"][..],
        &["-t", "inputs", "--"][..],
        &["-t", "inputs", "--", "-d"][..],
        &["-t", "inputs", "-dd"][..],
        &["-t", "inputs", "-dh"][..],
        &["-t", "inputs", "-dfoo"][..],
        &["-t", "multi-inputs", "-x"][..],
        &["-t", "multi-inputs", "--bogus"][..],
        &["-t", "multi-inputs", "--definitely-invalid"][..],
        &["-t", "multi-inputs", "--delimiter="][..],
        &["-t", "multi-inputs", "--delimiter"][..],
        &["-t", "multi-inputs", "-d"][..],
        &["-t", "multi-inputs", "--"][..],
        &["-t", "multi-inputs", "--", "-d"][..],
        &["-t", "compdb", "-z"][..],
        &["-t", "compdb-targets", "-z"][..],
        &["-t", "rules", "-x"][..],
        &["-t", "rules", "-dh"][..],
        &["-t", "rules", "-dfoo"][..],
        &["-t", "restat", "-x"][..],
        &["-t", "restat", "--bogus"][..],
        &["-t", "restat", "--help=ignored"][..],
        &["-t", "restat", "--builddir="][..],
        &["-t", "restat", "--builddir"][..],
        &["-t", "commands", "app", "-s"][..],
        &["-t", "commands", "other", "-ss"][..],
        &["-t", "commands", "--", "-x"][..],
        &["-t", "targets", "rule", ""][..],
        &["-t", "targets", "depth", "1trailing"][..],
        &["-t", "targets", "depth", "+2trailing"][..],
        &["-t", "rules", "ignored-operand", "-d"][..],
        &["-t", "clean", "app", "-gr"][..],
        &["-t", "compdb", "app", "-xx"][..],
        &["-t", "compdb-targets", "app", "-xx"][..],
        &["-t", "inputs", "other", "-00"][..],
        &["-t", "inputs", "other", "-0E"][..],
        &["-t", "inputs", "other", "-dd"][..],
        &["-t", "multi-inputs", "other", "-00"][..],
        &["-t", "deps", "source"][..],
        &["-t", "deps", "unknown"][..],
        &["-t", "query"][..],
        &["-t", "compdb-targets", "source"][..],
        &["-t", "compdb-targets", "unknown"][..],
    ] {
        let expected = run(ninja, temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>(),
            "{arguments:?}"
        );
        let expected_error = String::from_utf8_lossy(&expected.stderr).replace("ninja:", "tool:");
        let actual_error = String::from_utf8_lossy(&actual.stderr).replace("knight:", "tool:");
        assert_eq!(
            actual_error.lines().collect::<Vec<_>>(),
            expected_error.lines().collect::<Vec<_>>(),
            "{arguments:?}"
        );
    }
}

#[test]
fn remaining_tool_argument_surface_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cases = vec![
        vec!["-t", "query", "out", "source"],
        vec!["-t", "query", "missing"],
        vec!["-t", "targets"],
        vec!["-t", "targets", "depth"],
        vec!["-t", "targets", "depth", "0", "ignored"],
        vec!["-t", "targets", "rule", "cc", "ignored"],
        vec!["-t", "targets", "all", "ignored"],
        vec!["-t", "rules", "operand"],
        vec!["-t", "rules", "--", "-d"],
        vec!["-t", "commands"],
        vec!["-t", "commands", "-s"],
        vec!["-t", "inputs"],
        vec!["-t", "multi-inputs"],
        vec!["-t", "deps"],
        vec!["-t", "missingdeps"],
        vec!["-t", "recompact", "ignored"],
        vec!["-t", "restat"],
        vec!["-t", "cleandead", "ignored"],
        vec!["-t", "urtle", "ignored"],
        vec!["-t", "clean", "-g"],
        vec!["-t", "clean", "-r"],
    ];
    #[cfg(windows)]
    cases.extend([
        vec!["-t", "wincodepage"],
        vec!["-t", "wincodepage", "ignored"],
    ]);

    for arguments in cases {
        let mut outputs = Vec::new();
        for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                concat!(
                    "rule cc\n",
                    "  command = echo compile $in -o $out\n",
                    "  description = compile $out\n",
                    "build out: cc source\n",
                    "build other: cc unique\n",
                    "build app: phony out\n",
                    "default app\n",
                ),
            )
            .unwrap();
            fs::write(temp.path().join("source"), "source\n").unwrap();
            fs::write(temp.path().join("unique"), "unique\n").unwrap();
            fs::write(temp.path().join("out"), "output\n").unwrap();
            fs::write(temp.path().join("other"), "other output\n").unwrap();
            let alias = temp
                .path()
                .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
            if implementation == "knight" {
                install_ninja_alias(knight, &alias);
            }
            let executable = if implementation == "knight" {
                alias.as_path()
            } else {
                executable
            };
            outputs.push(run(executable, temp.path(), &arguments));
        }
        assert_eq!(
            outputs[1].status.code(),
            outputs[0].status.code(),
            "arguments={arguments:?}"
        );
        assert_eq!(
            outputs[1].stdout, outputs[0].stdout,
            "arguments={arguments:?}"
        );
        assert_eq!(
            outputs[1].stderr, outputs[0].stderr,
            "arguments={arguments:?}"
        );
    }
}

#[test]
fn ninja_tool_surface_inventory_is_complete() {
    const TOOLS: [(&str, &str, &str); 20] = [
        (
            "list",
            "none",
            "invocation_as_ninja_uses_ninja_diagnostic_identity",
        ),
        (
            "browse",
            "-p/--port, -a/--hostname, --no-browser, initial target",
            "browse_help_matches_ninjas_embedded_tool; browse_server_serves_query_pages_like_ninja",
        ),
        (
            "msvc",
            "-e, -o, -p, -h, --",
            "deprecated_msvc_helper_options_match_ninja_getopt; deprecated_msvc_helper_filters_output_and_writes_depfile",
        ),
        (
            "clean",
            "-g, -r, -h, targets/rules",
            "upstream_clean_all_target_rule_and_auxiliary_corpus_matches_ninja; tool_option_permutation_and_deps_target_errors_match_ninja",
        ),
        (
            "commands",
            "-s, -h, targets",
            "commands_and_compdb_options_match_ninja; tool_option_permutation_and_deps_target_errors_match_ninja",
        ),
        (
            "inputs",
            "-0/--print0, -E/--no-shell-escape, -d/--dependency-order, -h/--help",
            "inputs_tool_deduplicates_shared_inputs_across_targets; tool_option_permutation_and_deps_target_errors_match_ninja",
        ),
        (
            "multi-inputs",
            "-0/--print0, -d/--delimiter, -h/--help",
            "commands_and_compdb_options_match_ninja; tool_option_permutation_and_deps_target_errors_match_ninja",
        ),
        (
            "deps",
            "optional targets",
            "deps_tool_without_targets_uses_dependency_log_node_order_like_ninja; tool_option_permutation_and_deps_target_errors_match_ninja",
        ),
        (
            "missingdeps",
            "optional targets",
            "upstream_missing_dependency_scanner_corpus_matches_ninja_alias; missingdeps_without_targets_scans_only_default_closures_like_ninja",
        ),
        (
            "graph",
            "optional targets",
            "graph_tool_uses_ninja_graphviz_shape_and_implicit_defaults",
        ),
        (
            "query",
            "one or more paths",
            "remaining_tool_argument_surface_matches_ninja; graph_loads_only_reachable_dyndeps_and_warns_without_failing",
        ),
        (
            "targets",
            "rule [name], depth [n], all",
            "tool_target_modes_and_rule_descriptions_match_ninja; remaining_tool_argument_surface_matches_ninja",
        ),
        (
            "compdb",
            "-x, -h, optional rules",
            "commands_and_compdb_options_match_ninja; compdb_rsp_expansion_preserves_ninjas_first_marker_semantics",
        ),
        (
            "compdb-targets",
            "-x, -h, targets",
            "commands_and_compdb_options_match_ninja; compdb_keeps_outputs_used_as_both_validation_and_regular_inputs",
        ),
        (
            "recompact",
            "ignored operands",
            "recompact_discards_incompatible_metadata_without_recreating_it; remaining_tool_argument_surface_matches_ninja",
        ),
        (
            "restat",
            "--builddir, -h, optional outputs",
            "restat_compaction_missing_outputs_and_dry_run_match_ninja; tool_option_permutation_and_deps_target_errors_match_ninja",
        ),
        (
            "rules",
            "-d, -h",
            "tool_target_modes_and_rule_descriptions_match_ninja; remaining_tool_argument_surface_matches_ninja",
        ),
        (
            "cleandead",
            "ignored operands",
            "upstream_cleandead_removal_and_repeat_cases_match_ninja; remaining_tool_argument_surface_matches_ninja",
        ),
        (
            "urtle",
            "ignored operands",
            "remaining_tool_argument_surface_matches_ninja",
        ),
        (
            "wincodepage",
            "no operands",
            "remaining_tool_argument_surface_matches_ninja",
        ),
    ];
    let names = TOOLS
        .iter()
        .map(|(name, _, _)| *name)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(names.len(), TOOLS.len());
    assert!(
        TOOLS
            .iter()
            .all(|(_, arguments, evidence)| !arguments.is_empty() && !evidence.is_empty())
    );
}

#[test]
fn ninja_top_level_option_surface_inventory_is_complete() {
    const OPTIONS: [(&str, &str); 15] = [
        ("-C", "invocation_as_ninja_uses_ninja_diagnostic_identity"),
        ("-d", "invocation_as_ninja_uses_ninja_diagnostic_identity"),
        (
            "-f",
            "nested_include_paths_resolve_from_the_working_directory_like_ninja",
        ),
        ("-j", "top_level_short_option_clusters_match_ninja"),
        ("-k", "top_level_short_option_clusters_match_ninja"),
        ("-l", "top_level_short_option_clusters_match_ninja"),
        ("-n", "dry_run_starts_the_entire_ready_frontier_like_ninja"),
        ("-t", "ninja_tool_surface_inventory_is_complete"),
        (
            "-v/--verbose",
            "top_level_short_option_clusters_match_ninja",
        ),
        ("-w", "invocation_as_ninja_uses_ninja_diagnostic_identity"),
        (
            "-h/--help",
            "top_level_help_uses_ninja_exit_status_and_stream",
        ),
        (
            "--quiet",
            "attached_long_option_values_follow_platform_getopt_semantics",
        ),
        (
            "--status",
            "explicit_status_format_matches_ninja_and_counts_only_dirty_edges",
        ),
        (
            "--version",
            "invocation_as_ninja_uses_ninja_diagnostic_identity",
        ),
        ("--", "posixly_correct_stops_option_permutation_like_ninja"),
    ];
    let names = OPTIONS
        .iter()
        .map(|(name, _)| *name)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(names.len(), OPTIONS.len());
    assert!(OPTIONS.iter().all(|(_, evidence)| !evidence.is_empty()));
}

#[test]
fn posixly_correct_stops_option_permutation_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let alias = temp
        .path()
        .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
    install_ninja_alias(knight, &alias);
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n  command = echo cc $in -o $out\n  description = CC $out\n",
            "build out: cc source\n",
            "build app: phony out\n",
            "build other: cc unique\n",
            "default app\n",
        ),
    )
    .unwrap();

    for arguments in [
        &["out", "-n"][..],
        &["out", "-v"][..],
        &["-C", ".", "out", "-n"][..],
        &["--status", "x", "out", "-n"][..],
        &["-t", "commands", "other", "-s"][..],
        &["-t", "rules", "operand", "-d"][..],
        &["-t", "clean", "app", "-g"][..],
        &["-t", "compdb", "cc", "-x"][..],
        &["-t", "compdb-targets", "other", "-x"][..],
        &["-t", "inputs", "other", "-0"][..],
        &["-t", "multi-inputs", "other", "-0"][..],
        &["-t", "restat", "out", "-h"][..],
    ] {
        let run_ordered = |executable: &Path| {
            Command::new(executable)
                .current_dir(temp.path())
                .args(arguments)
                .env("POSIXLY_CORRECT", "1")
                .output()
                .unwrap()
        };
        let expected = run_ordered(Path::new(&ninja));
        let actual = run_ordered(&alias);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        assert_eq!(actual.stdout, expected.stdout, "{arguments:?}");
        assert_eq!(actual.stderr, expected.stderr, "{arguments:?}");
    }
}

#[test]
fn compdb_keeps_outputs_used_as_both_validation_and_regular_inputs() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n  command = cc -c $in -o $out\n",
            "rule link\n  command = cc $in -o $out\n",
            "build foo.o: cc foo.c |@ bar.o\n",
            "build bar.o: cc bar.c\n",
            "build app: link foo.o bar.o\n",
        ),
    )
    .unwrap();

    for arguments in [
        &["-t", "compdb"][..],
        &["-t", "compdb-targets", "foo.o"][..],
    ] {
        let expected = run(Path::new(&ninja), temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert!(expected.status.success() && actual.status.success());
        let expected_json: serde_json::Value = serde_json::from_slice(&expected.stdout).unwrap();
        let actual_json: serde_json::Value = serde_json::from_slice(&actual.stdout).unwrap();
        assert_eq!(actual_json, expected_json, "arguments={arguments:?}");
    }

    fs::write(
        temp.path().join("build.ninja"),
        "rule stamp\n  command = stamp > $out\nbuild empty: stamp\n",
    )
    .unwrap();
    let arguments = ["-t", "compdb-targets", "empty"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&expected.stdout)
            .lines()
            .collect::<Vec<_>>()
    );
}

#[test]
fn inputs_tool_deduplicates_shared_inputs_across_targets() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n  command = cc $in -o $out\n",
            "build a: cc shared source-a path$ with$ space\n",
            "build b: cc shared source-b\n",
        ),
    )
    .unwrap();

    for arguments in [
        &["-t", "inputs", "a", "b"][..],
        &["-t", "inputs", "-d", "a", "b"][..],
        &["-t", "inputs", "-E", "a", "b"][..],
    ] {
        let expected = run(Path::new(&ninja), temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert!(expected.status.success() && actual.status.success());
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>(),
            "arguments={arguments:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .filter(|line| *line == "shared")
                .count(),
            1,
            "arguments={arguments:?}"
        );
    }
}

#[test]
fn successful_missing_outputs_do_not_block_dependents_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        let command = if cfg!(windows) {
            "cmd /d /c echo built-$out"
        } else {
            "printf 'built-$out\\n'"
        };
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                concat!(
                    "rule announce\n  command = {}\n",
                    "build absent: announce\n",
                    "build final: announce absent\n",
                    "default final\n",
                ),
                command
            ),
        )
        .unwrap();
        for invocation in 0..2 {
            let output = run(executable, temp.path(), &["-j1"]);
            assert!(
                output.status.success(),
                "invocation={invocation} executable={} stdout={} stderr={}",
                executable.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("built-absent"), "stdout={stdout}");
            assert!(stdout.contains("built-final"), "stdout={stdout}");
        }
    }
}

#[test]
fn phony_targets_reject_missing_source_inputs_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "build all: phony missing\ndefault all\n",
    )
    .unwrap();

    for arguments in [&[][..], &["-n"][..]] {
        let expected = run(Path::new(&ninja), temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert!(!actual.status.success());
        assert!(String::from_utf8_lossy(&actual.stderr).contains("missing"));
    }
}

#[cfg(unix)]
#[test]
fn filesystem_stat_failures_abort_before_missing_input_diagnostics() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let input = "i".repeat(400);
    fs::write(
        temp.path().join("build.ninja"),
        format!("build out: phony {input}\n"),
    )
    .unwrap();

    for arguments in [&["-n"][..], &["-d", "nostatcache", "-n"][..]] {
        let expected = run(Path::new(&ninja), temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(
            String::from_utf8_lossy(&actual.stderr).replace("knight:", "tool:"),
            String::from_utf8_lossy(&expected.stderr).replace("ninja:", "tool:")
        );
    }
}

#[cfg(unix)]
#[test]
fn deps_tool_reports_but_ignores_output_stat_failures_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let output = "o".repeat(400);
    fs::write(
        temp.path().join("build.ninja"),
        format!("rule cc\n  command = cc\n  deps = gcc\nbuild {output}: cc\n"),
    )
    .unwrap();

    let mut deps_log = b"# ninjadeps\n".to_vec();
    deps_log.extend_from_slice(&4u32.to_le_bytes());
    let padding = (4 - output.len() % 4) % 4;
    deps_log.extend_from_slice(&((output.len() + padding + 4) as u32).to_le_bytes());
    deps_log.extend_from_slice(output.as_bytes());
    deps_log.extend_from_slice(&[0; 3][..padding]);
    deps_log.extend_from_slice(&u32::MAX.to_le_bytes());
    deps_log.extend_from_slice(&0x8000_000cu32.to_le_bytes());
    deps_log.extend_from_slice(&0u32.to_le_bytes());
    deps_log.extend_from_slice(&1u32.to_le_bytes());
    deps_log.extend_from_slice(&0u32.to_le_bytes());
    fs::write(temp.path().join(".ninja_deps"), deps_log).unwrap();

    let arguments = ["-t", "deps", output.as_str()];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(knight, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(
        String::from_utf8_lossy(&actual.stderr).replace("knight:", "tool:"),
        String::from_utf8_lossy(&expected.stderr).replace("ninja:", "tool:")
    );
}

#[cfg(unix)]
#[test]
fn post_command_stat_failures_stop_builds_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let root = tempdir().unwrap();
    let alias = root.path().join("ninja");
    install_ninja_alias(knight, &alias);

    for (name, bindings, depfile_command) in [
        (
            "deps",
            "  deps = gcc\n  depfile = out.d\n",
            " && printf 'out:' > out.d",
        ),
        ("restat", "  restat = 1\n", ""),
        ("generator", "  generator = 1\n", ""),
    ] {
        let manifest = format!(
            "rule bad\n  command = ln -s out out{depfile_command}\n{bindings}build out: bad\ndefault out\n"
        );
        let mut expected = None;
        for (variant, executable) in [("expected", Path::new(&ninja)), ("actual", &alias)] {
            let directory = root.path().join(format!("{name}-{variant}"));
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("build.ninja"), &manifest).unwrap();
            let result = run(executable, &directory, &[]);
            let result = (result.status.code(), result.stdout, result.stderr);
            if let Some(expected) = &expected {
                assert_eq!(&result, expected, "{name}");
            } else {
                expected = Some(result);
            }
        }
    }
}

#[test]
fn missing_order_only_source_inputs_follow_phony_policy_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for phony in [true, false] {
        let temp = tempdir().unwrap();
        let manifest = if phony {
            "build out: phony || missing\ndefault out\n".to_owned()
        } else {
            let command = if cfg!(windows) {
                "cmd /c echo out"
            } else {
                "echo out"
            };
            format!("rule make\n  command = {command}\nbuild out: make || missing\ndefault out\n")
        };
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let expected = run(Path::new(&ninja), temp.path(), &["-n"]);
        let actual = run(knight, temp.path(), &["-n"]);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "phony={phony}"
        );
        if phony {
            assert_eq!(
                String::from_utf8_lossy(&actual.stdout)
                    .replace("knight:", "tool:")
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
                String::from_utf8_lossy(&expected.stdout)
                    .replace("ninja:", "tool:")
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
                "phony={phony}"
            );
        } else {
            assert!(!actual.status.success());
            assert!(String::from_utf8_lossy(&actual.stderr).contains("missing"));
        }
    }

    let temp = tempdir().unwrap();
    let command = if cfg!(windows) {
        "cmd /d /c echo built>$out"
    } else {
        "printf built > $out"
    };
    fs::write(
        temp.path().join("build.ninja"),
        format!(
            "rule make\n  command = {command}\n\
             build generated: make\n\
             build out: phony || missing generated\n\
             default out\n"
        ),
    )
    .unwrap();
    for arguments in [&[][..], &["-n"][..]] {
        for executable in [Path::new(&ninja), knight] {
            let result = run(executable, temp.path(), arguments);
            assert!(!result.status.success(), "arguments={arguments:?}");
            assert!(result.stdout.is_empty(), "arguments={arguments:?}");
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("missing"),
                "arguments={arguments:?}"
            );
        }
    }
}

#[test]
fn dry_run_reports_no_work_for_commandless_targets_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "build all: phony\ndefault all\n",
    )
    .unwrap();
    let expected = run(Path::new(&ninja), temp.path(), &["-n"]);
    let actual = run(knight, temp.path(), &["-n"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .replace("knight:", "tool:")
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&expected.stdout)
            .replace("ninja:", "tool:")
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    );
}

#[test]
fn dry_run_starts_the_entire_ready_frontier_like_ninja() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule run\n  command = echo $out\n",
            "build root: run\n",
            "build earlier: run | root\n",
            "build later: run\n",
            "build final: run later || earlier\n",
            "build phony_root: run\n",
            "build after_command: run phony_root\n",
            "build gate: phony\n",
            "build after_phony: run gate\n",
            "build final_phony: phony after_command after_phony\n",
            "default final\n",
        ),
    )
    .unwrap();

    let arguments = ["-n", "-j1"];
    let actual = run(knight, temp.path(), &arguments);
    assert!(actual.status.success());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        [
            "[1/4] echo root",
            "[2/4] echo later",
            "[3/4] echo earlier",
            "[4/4] echo final",
        ]
    );

    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        let expected = run(Path::new(&ninja), temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>()
        );
    }

    let phony_arguments = ["-n", "-j1", "final_phony"];
    let phony_actual = run(knight, temp.path(), &phony_arguments);
    assert!(phony_actual.status.success());
    assert_eq!(
        String::from_utf8_lossy(&phony_actual.stdout)
            .lines()
            .collect::<Vec<_>>(),
        [
            "[1/3] echo phony_root",
            "[2/3] echo after_phony",
            "[3/3] echo after_command",
        ]
    );
    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        let expected = run(Path::new(&ninja), temp.path(), &phony_arguments);
        assert_eq!(phony_actual.status.code(), expected.status.code());
        assert_eq!(
            String::from_utf8_lossy(&phony_actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn invocation_as_ninja_uses_ninja_diagnostic_identity() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let alias = temp
        .path()
        .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
    install_ninja_alias(knight, &alias);
    fs::create_dir(temp.path().join("project")).unwrap();
    fs::write(
        temp.path().join("project/build.ninja"),
        "build all: phony\ndefault all\n",
    )
    .unwrap();

    for arguments in [
        &["--help"][..],
        &["--version"][..],
        &["-d", "list"][..],
        &["-w", "list"][..],
        &["-t", "list"][..],
    ] {
        let expected = run(Path::new(&ninja), temp.path(), arguments);
        let actual = run(&alias, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        assert_eq!(actual.stdout, expected.stdout, "{arguments:?}");
        assert_eq!(actual.stderr, expected.stderr, "{arguments:?}");
    }

    let arguments = ["-C", "project"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(&alias, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);

    let arguments = ["-t", "targts"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(&alias, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);

    for arguments in [
        &["-j", "not-a-number"][..],
        &["-j", "-1"][..],
        &["-k", "not-a-number"][..],
        &["-l", "not-a-number"][..],
    ] {
        let expected = run(Path::new(&ninja), temp.path(), arguments);
        let actual = run(&alias, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        assert_eq!(actual.stdout, expected.stdout, "{arguments:?}");
        assert_eq!(actual.stderr, expected.stderr, "{arguments:?}");
    }

    let fail_command = if cfg!(windows) {
        "cmd /d /c exit 7"
    } else {
        "sh -c 'exit 7'"
    };
    fs::write(
        temp.path().join("project/build.ninja"),
        format!("rule fail\n  command = {fail_command}\nbuild failed: fail\n"),
    )
    .unwrap();
    let arguments = ["-C", "project", "failed"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(&alias, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);

    let (fail_seven, fail_nine) = if cfg!(windows) {
        ("cmd /d /c exit 7", "cmd /d /c exit 9")
    } else {
        ("sh -c 'exit 7'", "sh -c 'exit 9'")
    };
    fs::write(
        temp.path().join("project/build.ninja"),
        format!(
            "rule f7\n  command = {fail_seven}\nrule f9\n  command = {fail_nine}\nbuild a: f7\nbuild b: f9\nbuild all: phony a b\n"
        ),
    )
    .unwrap();
    let arguments = ["-C", "project", "-j1", "-k0", "all"];
    let expected = run(Path::new(&ninja), temp.path(), &arguments);
    let actual = run(&alias, temp.path(), &arguments);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn invocation_as_ninja_matches_jobserver_makeflags_diagnostics() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    let alias = temp
        .path()
        .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
    install_ninja_alias(knight, &alias);
    fs::write(
        temp.path().join("build.ninja"),
        "build all: phony\ndefault all\n",
    )
    .unwrap();

    let unsupported_platform = if cfg!(windows) {
        "--jobserver-auth=fifo:missing"
    } else {
        "--jobserver-auth=missing-semaphore"
    };
    let supported_missing = if cfg!(windows) {
        "--jobserver-auth=missing-semaphore"
    } else {
        "--jobserver-auth=fifo:/definitely/missing/knight-jobserver"
    };
    for (makeflags, arguments) in [
        ("--jobserver-fds=10,", &[][..]),
        ("--jobserver-auth=10,42", &[][..]),
        (unsupported_platform, &[][..]),
        (supported_missing, &[][..]),
        ("kns --jobserver-auth=10,42", &[][..]),
        ("--jobserver-auth=10,42", &["--quiet"][..]),
        ("--jobserver-auth=10,42", &["-n"][..]),
    ] {
        let invoke = |executable: &Path| {
            Command::new(executable)
                .current_dir(temp.path())
                .args(arguments)
                .env("MAKEFLAGS", makeflags)
                .env_remove("CARGO_MAKEFLAGS")
                .env_remove("MFLAGS")
                .output()
                .unwrap()
        };
        let expected = invoke(Path::new(&ninja));
        let actual = invoke(&alias);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{makeflags} {arguments:?}"
        );
        assert_eq!(actual.stdout, expected.stdout, "{makeflags} {arguments:?}");
        assert_eq!(actual.stderr, expected.stderr, "{makeflags} {arguments:?}");
    }

    #[cfg(windows)]
    {
        let client = jobserver::Client::new(1).unwrap();
        let invoke = |executable: &Path| {
            let mut command = Command::new(executable);
            command.current_dir(temp.path());
            client.configure_make(&mut command);
            command
                .env_remove("CARGO_MAKEFLAGS")
                .env_remove("MFLAGS")
                .output()
                .unwrap()
        };
        let expected = invoke(Path::new(&ninja));
        let actual = invoke(&alias);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
    }
}

#[cfg(unix)]
#[test]
fn explain_reports_the_dirty_dependency_that_triggers_each_edge() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let expected_dir = tempdir().unwrap();
    let actual_dir = tempdir().unwrap();
    let manifest = concat!(
        "build .FORCE: phony\n",
        "rule create\n  command = [ -e $out ] || touch $out\n  restat = true\n",
        "rule copy\n  command = cp $in $out\n",
        "build input: create .FORCE\n",
        "build mid: copy input\n",
        "build output: copy mid\n",
        "default output\n",
    );
    fs::write(expected_dir.path().join("build.ninja"), manifest).unwrap();
    fs::write(actual_dir.path().join("build.ninja"), manifest).unwrap();

    for invocation in 0..2 {
        let expected = run(
            Path::new(&ninja),
            expected_dir.path(),
            &["-v", "-d", "explain"],
        );
        let actual = run(knight, actual_dir.path(), &["-v", "-d", "explain"]);
        assert!(expected.status.success() && actual.status.success());
        assert_eq!(actual.stdout, expected.stdout, "invocation={invocation}");
        assert_eq!(
            String::from_utf8_lossy(&actual.stderr).replace("knight explain:", "build explain:"),
            String::from_utf8_lossy(&expected.stderr).replace("ninja explain:", "build explain:"),
            "invocation={invocation}"
        );
    }
}

#[cfg(unix)]
#[test]
fn explain_reports_dyndep_loads_without_duplicate_output_reasons() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule scan\n",
            "  command = printf 'ninja_dyndep_version = 1\\nbuild out | out.imp: dyndep\\n' > $out\n",
            "rule touch\n  command = touch $out $out.imp\n",
            "build dd: scan\n",
            "build out: touch || dd\n  dyndep = dd\n",
            "default out\n",
        ),
    )
    .unwrap();

    let first = run(knight, temp.path(), &["-v", "-d", "explain"]);
    assert!(first.status.success());
    assert_eq!(
        String::from_utf8_lossy(&first.stderr)
            .lines()
            .collect::<Vec<_>>(),
        [
            "knight explain: output dd doesn't exist",
            "knight explain: loading dyndep file 'dd'",
            "knight explain: output out doesn't exist",
        ]
    );

    let second = run(knight, temp.path(), &["-v", "-d", "explain"]);
    assert!(second.status.success());
    assert_eq!(
        String::from_utf8_lossy(&second.stderr)
            .lines()
            .collect::<Vec<_>>(),
        ["knight explain: loading dyndep file 'dd'"]
    );
    assert!(String::from_utf8_lossy(&second.stdout).contains("no work"));
}

#[cfg(target_os = "linux")]
#[test]
fn smart_terminal_status_and_output_framing_match_ninja() {
    fn shell_word(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }

    fn run_in_pty(executable: &Path, directory: &Path, arguments: &[&str]) -> Output {
        let command = std::iter::once(executable.to_string_lossy().into_owned())
            .chain(arguments.iter().map(|argument| (*argument).to_owned()))
            .map(|argument| shell_word(&argument))
            .collect::<Vec<_>>()
            .join(" ");
        Command::new("script")
            .current_dir(directory)
            .args(["-qfec", &command, "/dev/null"])
            .env("TERM", "")
            .output()
            .unwrap()
    }

    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule emit\n",
            "  command = printf 'command-output'\n",
            "  description = build $out\n",
            "build a: emit\n",
            "build b: emit a\n",
            "default b\n",
        ),
    )
    .unwrap();

    for arguments in [
        &[][..],
        &["--quiet"][..],
        &["-v"][..],
        &["-n"][..],
        &["-n", "-v"][..],
        &["--status", "<$finished/$total> $description"][..],
    ] {
        let expected = run_in_pty(Path::new(&ninja), temp.path(), arguments);
        let actual = run_in_pty(knight, temp.path(), arguments);
        assert!(expected.status.success() && actual.status.success());
        assert_eq!(actual.stdout, expected.stdout, "arguments={arguments:?}");
        assert_eq!(actual.stderr, expected.stderr, "arguments={arguments:?}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn console_pool_inherits_all_terminal_descriptors_like_ninja() {
    fn shell_word(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }

    fn run_in_pty(executable: &Path, directory: &Path) -> Output {
        let command = shell_word(&executable.to_string_lossy());
        Command::new("script")
            .current_dir(directory)
            .args(["-qfec", &command, "/dev/null"])
            .env("TERM", "")
            .output()
            .unwrap()
    }

    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let expected_dir = tempdir().unwrap();
    let actual_dir = tempdir().unwrap();
    let manifest = concat!(
        "rule console\n",
        "  command = test -t 0 -a -t 1 -a -t 2\n",
        "  pool = console\n",
        "build out: console\n",
        "default out\n",
    );
    fs::write(expected_dir.path().join("build.ninja"), manifest).unwrap();
    fs::write(actual_dir.path().join("build.ninja"), manifest).unwrap();
    let expected = run_in_pty(Path::new(&ninja), expected_dir.path());
    let actual = run_in_pty(knight, actual_dir.path());
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[cfg(target_os = "linux")]
#[test]
fn smart_terminal_status_elision_matches_ninja() {
    fn shell_word(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }

    fn run_in_pty(executable: &Path, directory: &Path, arguments: &[&str], width: usize) -> Output {
        let command = std::iter::once(executable.to_string_lossy().into_owned())
            .chain(arguments.iter().map(|argument| (*argument).to_owned()))
            .map(|argument| shell_word(&argument))
            .collect::<Vec<_>>()
            .join(" ");
        Command::new("script")
            .current_dir(directory)
            .args([
                "-qfec",
                &format!("stty cols {width}; {command}"),
                "/dev/null",
            ])
            .env("TERM", "xterm")
            .output()
            .unwrap()
    }

    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    for (name, description) in [
        ("plain", "012345678901234567890123456789"),
        ("ansi", "012345\x1b[0;35m678901234567890123456789\x1b[0m"),
    ] {
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                "rule run\n  command = echo 012345678901234567890123456789\n  description = {description}\nbuild out: run\ndefault out\n"
            ),
        )
        .unwrap();
        for arguments in [&["-n", "-j1"][..], &["-n", "-j1", "-v"][..]] {
            let expected = run_in_pty(Path::new(&ninja), temp.path(), arguments, 20);
            let actual = run_in_pty(knight, temp.path(), arguments, 20);
            assert!(expected.status.success() && actual.status.success());
            assert_eq!(actual.stdout, expected.stdout, "{name} {arguments:?}");
            assert_eq!(actual.stderr, expected.stderr, "{name} {arguments:?}");
        }
    }
}

#[test]
fn top_level_short_option_clusters_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "rule echo\n  command = echo built > $out\nbuild out: echo\n",
    )
    .unwrap();

    for arguments in [
        &["-nvj1"][..],
        &["-fbuild.ninja", "-n"][..],
        &["-k-1", "-tlist"][..],
        &["-j", "  +1", "-n", "out"][..],
        &["-j", "-0", "-n", "out"][..],
        &["-j999999999999999999999999999999999999", "-n", "out"][..],
        &["-k", "  +1", "-n", "out"][..],
        &["-k-999999999999999999999999999999999999", "-n", "out"][..],
        &["-l-1", "-tlist"][..],
        &["-tlist"][..],
        &["-tinputs", "-0Ed"][..],
        &["-ntinputs", "-dd"][..],
    ] {
        let expected = run(ninja, temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>(),
            "{arguments:?}"
        );
    }
}

#[test]
fn long_option_abbreviations_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "rule echo\n  command = echo built > $out\nbuild out: echo input\ndefault out\n",
    )
    .unwrap();
    fs::write(temp.path().join("input"), "input\n").unwrap();

    for arguments in [
        &["--verb", "-n"][..],
        &["--qui", "-n"][..],
        &["--sta", "<$finished/$total> $description", "-n"][..],
        &["-l1trailing", "-tlist"][..],
        &["-t", "inputs", "--dependency-o", "out"][..],
        &["-t", "inputs", "--no-shell", "out"][..],
        &["-t", "multi-inputs", "--delim=::", "out"][..],
    ] {
        let expected = run(ninja, temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout).replace("\r\n", "\n"),
            String::from_utf8_lossy(&expected.stdout).replace("\r\n", "\n"),
            "{arguments:?}"
        );
        assert_eq!(actual.stderr, expected.stderr, "{arguments:?}");
    }
}

#[test]
fn attached_long_option_values_follow_platform_getopt_semantics() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "build all: phony\ndefault all\n",
    )
    .unwrap();

    for arguments in [
        &["--version=ignored"][..],
        &["--help=ignored"][..],
        &["--quiet=ignored", "-n"][..],
        &["--verbose=ignored", "-n"][..],
        &["--status="][..],
    ] {
        let expected = run(ninja, temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        if matches!(arguments[0], "--quiet=ignored" | "--verbose=ignored") {
            let expected_output = String::from_utf8_lossy(&expected.stdout)
                .replace("ninja:", "tool:")
                .replace("\r\n", "\n");
            let actual_output = String::from_utf8_lossy(&actual.stdout)
                .replace("knight:", "tool:")
                .replace("\r\n", "\n");
            assert_eq!(actual_output, expected_output, "{arguments:?}");
        }
        assert_eq!(
            actual.stdout.is_empty(),
            expected.stdout.is_empty(),
            "{arguments:?}"
        );
        assert_eq!(
            actual.stderr.is_empty(),
            expected.stderr.is_empty(),
            "{arguments:?}"
        );
    }
}

#[test]
fn graph_tool_uses_ninja_graphviz_shape_and_implicit_defaults() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "build a: phony source\nbuild final: phony a\n",
    )
    .unwrap();
    let actual = run(knight, temp.path(), &["-t", "graph"]);
    assert!(actual.status.success());
    let graph = String::from_utf8(actual.stdout).unwrap();
    assert_eq!(
        graph.lines().take(2).collect::<Vec<_>>(),
        ["digraph ninja {", "rankdir=\"LR\""]
    );
    assert!(graph.contains("\"a\" -> \"final\" [label=\" phony\"]"));
    assert!(graph.contains("\"source\" -> \"a\" [label=\" phony\"]"));
    assert!(!graph.contains("shape=ellipse"));
}

#[test]
fn upstream_graph_dirty_state_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let cases = [
        (
            "missing implicit input",
            "build out: cat in | implicit\n",
            &["in", "out"][..],
            &[][..],
        ),
        (
            "modified implicit input",
            "build out: cat in | implicit\n",
            &["in", "out"][..],
            &["implicit"][..],
        ),
        (
            "missing implicit output",
            "build out | out.imp: cat in\n",
            &["in", "out"][..],
            &[][..],
        ),
        (
            "outdated implicit output",
            "build out | out.imp: cat in\n",
            &["out.imp"][..],
            &["in", "out"][..],
        ),
        (
            "missing implicit-only output",
            "build | out.imp: cat in\n",
            &["in"][..],
            &[][..],
        ),
        (
            "outdated implicit-only output",
            "build | out.imp: cat in\n",
            &["out.imp"][..],
            &["in"][..],
        ),
    ];

    for (name, edge, older, newer) in cases {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!("rule cat\n  command = echo cat $in $out\n{edge}"),
        )
        .unwrap();
        for path in older {
            fs::write(temp.path().join(path), "older\n").unwrap();
        }
        if !newer.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        for path in newer {
            fs::write(temp.path().join(path), "newer\n").unwrap();
        }
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        install_ninja_alias(knight, &alias);
        let target = if edge.contains("build |") {
            "out.imp"
        } else {
            "out"
        };
        let expected = run(ninja, temp.path(), &["-n", "-v", target]);
        let actual = run(&alias, temp.path(), &["-n", "-v", target]);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }
}

#[test]
fn upstream_graph_depfile_path_and_removal_cases_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let manifest = concat!(
        "rule catdep\n",
        "  depfile = $out.d\n",
        "  command = echo cat $in $out\n",
        "build ./out.o: catdep ./foo.cc\n",
    );

    for (name, depfile, newer) in [
        ("canonical declared path", "out.o: bar/../foo.cc\n", None),
        (
            "canonical discovered path",
            "out.o: ./foo/../implicit.h\n",
            Some("implicit.h"),
        ),
    ] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        fs::write(temp.path().join("foo.cc"), "source\n").unwrap();
        fs::write(temp.path().join("out.o.d"), depfile).unwrap();
        fs::write(temp.path().join("out.o"), "output\n").unwrap();
        if let Some(newer) = newer {
            std::thread::sleep(std::time::Duration::from_millis(20));
            fs::write(temp.path().join(newer), "newer\n").unwrap();
        }
        let expected = run(ninja, temp.path(), &["-n", "-v", "out.o"]);
        let actual = run(knight, temp.path(), &["-n", "-v", "out.o"]);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }

    let temp = tempdir().unwrap();
    fs::write(temp.path().join("build.ninja"), manifest).unwrap();
    fs::write(temp.path().join("foo.cc"), "source\n").unwrap();
    fs::write(temp.path().join("foo.h"), "header\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(temp.path().join("out.o.d"), "out.o: foo.h\n").unwrap();
    fs::write(temp.path().join("out.o"), "output\n").unwrap();
    for (name, remove_depfile) in [("present", false), ("removed", true)] {
        if remove_depfile {
            fs::remove_file(temp.path().join("out.o.d")).unwrap();
        }
        let expected = run(ninja, temp.path(), &["-n", "-v", "out.o"]);
        let actual = run(knight, temp.path(), &["-n", "-v", "out.o"]);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }
}

#[test]
fn upstream_graph_collectors_and_path_escaping_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cat\n  command = cat $in > $out\n",
            "build out1: cat in1\n",
            "build mid1: cat in1\n",
            "build out2: cat mid1\n",
            "build out3 out4: cat mid1\n",
            "build all: phony out1 out2 out3\n",
            "build out$ 1: cat in1 in2 in$ with$ space | implicit || order_only\n",
            "build a$ b: cat no'space with$ space$$ no\"space2\n",
        ),
    )
    .unwrap();

    for arguments in [
        &["-t", "inputs", "out1"][..],
        &["-t", "inputs", "out2"][..],
        &["-t", "inputs", "all"][..],
        &["-t", "inputs", "out 1"][..],
        &["-t", "commands", "out2"][..],
        &["-t", "commands", "all"][..],
        &["-t", "commands", "a b"][..],
    ] {
        let expected = run(ninja, temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{arguments:?}"
        );
        assert_eq!(actual.stdout, expected.stdout, "{arguments:?}");
        assert_eq!(actual.stderr, expected.stderr, "{arguments:?}");
    }
}

#[test]
fn upstream_graph_binding_and_dependency_type_cases_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    for (name, manifest) in [
        (
            "rule variable scope",
            "rule r\n  depfile = x\n  command = depfile is $depfile\nbuild out: r in\n",
        ),
        (
            "depfile override",
            "rule r\n  depfile = x\n  command = depfile is $depfile\nbuild out: r in\n  depfile = y\n",
        ),
        (
            "nested phony",
            "build n1: phony\nbuild n2: phony n1\ndefault n2\n",
        ),
    ] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        install_ninja_alias(knight, &alias);
        for arguments in [&["-t", "commands"][..], &["-n", "-v"][..]] {
            let expected = run(ninja, temp.path(), arguments);
            let actual = run(&alias, temp.path(), arguments);
            assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
            assert_eq!(actual.stdout, expected.stdout, "case={name}");
            assert_eq!(actual.stderr, expected.stderr, "case={name}");
        }
    }

    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cat\n  command = echo cat $in $out\n",
            "rule catdep\n  depfile = $out.d\n  command = echo catdep $in $out\n",
            "build implicit.h: cat data\n",
            "build out.o: catdep foo.cc || implicit.h\n",
        ),
    )
    .unwrap();
    for path in ["implicit.h", "foo.cc", "out.o"] {
        fs::write(temp.path().join(path), "older\n").unwrap();
    }
    fs::write(temp.path().join("out.o.d"), "out.o: implicit.h\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(temp.path().join("data"), "newer\n").unwrap();
    let expected = run(ninja, temp.path(), &["-n", "-v", "out.o"]);
    let actual = run(knight, temp.path(), &["-n", "-v", "out.o"]);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn upstream_graph_case_inventory_is_complete() {
    let cases = [
        (
            "MissingImplicit",
            "upstream_graph_dirty_state_corpus_matches_ninja",
        ),
        (
            "ModifiedImplicit",
            "upstream_graph_dirty_state_corpus_matches_ninja",
        ),
        (
            "FunkyMakefilePath",
            "upstream_graph_depfile_path_and_removal_cases_match_ninja",
        ),
        (
            "ExplicitImplicit",
            "upstream_graph_binding_and_dependency_type_cases_match_ninja",
        ),
        (
            "ImplicitOutputParse",
            "upstream_manifest_parser_semantic_corpus",
        ),
        (
            "ImplicitOutputMissing",
            "upstream_graph_dirty_state_corpus_matches_ninja",
        ),
        (
            "ImplicitOutputOutOfDate",
            "upstream_graph_dirty_state_corpus_matches_ninja",
        ),
        (
            "ImplicitOutputOnlyParse",
            "upstream_manifest_parser_semantic_corpus",
        ),
        (
            "ImplicitOutputOnlyMissing",
            "upstream_graph_dirty_state_corpus_matches_ninja",
        ),
        (
            "ImplicitOutputOnlyOutOfDate",
            "upstream_graph_dirty_state_corpus_matches_ninja",
        ),
        (
            "PathWithCurrentDirectory",
            "upstream_graph_depfile_path_and_removal_cases_match_ninja",
        ),
        ("RootNodes", "upstream_default_node_selection_matches_ninja"),
        (
            "InputsCollector",
            "upstream_graph_collectors_and_path_escaping_match_ninja",
        ),
        (
            "InputsCollectorWithEscapes",
            "upstream_graph_collectors_and_path_escaping_match_ninja",
        ),
        (
            "CommandCollector",
            "upstream_graph_collectors_and_path_escaping_match_ninja",
        ),
        (
            "VarInOutPathEscaping",
            "upstream_graph_collectors_and_path_escaping_match_ninja",
        ),
        (
            "DepfileWithCanonicalizablePath",
            "upstream_graph_depfile_path_and_removal_cases_match_ninja",
        ),
        (
            "DepfileRemoved",
            "upstream_graph_depfile_path_and_removal_cases_match_ninja",
        ),
        (
            "RuleVariablesInScope",
            "upstream_graph_binding_and_dependency_type_cases_match_ninja",
        ),
        (
            "DepfileOverride",
            "upstream_graph_binding_and_dependency_type_cases_match_ninja",
        ),
        (
            "DepfileOverrideParent",
            "upstream_graph_binding_and_dependency_type_cases_match_ninja",
        ),
        (
            "NestedPhonyPrintsDone",
            "upstream_graph_binding_and_dependency_type_cases_match_ninja",
        ),
        (
            "PhonySelfReferenceError",
            "phony_self_reference_policy_matches_ninja_tools",
        ),
        ("DependencyCycle", "rootless_graph_error_matches_ninja"),
        (
            "CycleInEdgesButNotInNodes1",
            "multi_output_edges_retain_real_self_cycles_like_ninja",
        ),
        (
            "CycleInEdgesButNotInNodes2",
            "multi_output_edges_retain_real_self_cycles_like_ninja",
        ),
        (
            "CycleInEdgesButNotInNodes3",
            "multi_output_edges_retain_real_self_cycles_like_ninja",
        ),
        (
            "CycleInEdgesButNotInNodes4",
            "multi_output_edges_retain_real_self_cycles_like_ninja",
        ),
        (
            "CycleWithLengthZeroFromDepfile",
            "stale_depfile_cycle_is_ignored_when_declared_inputs_are_dirty",
        ),
        (
            "ManifestInputDirtyNoDepfileLoad",
            "manifest_dirty_edges_do_not_load_stale_discovered_dependencies",
        ),
        (
            "CycleWithLengthOneFromDepfile",
            "stale_depfile_failures_match_ninja",
        ),
        (
            "CycleWithLengthOneFromDepfileOneHopAway",
            "stale_depfile_failures_match_ninja",
        ),
        (
            "Decanonicalize",
            "command_path_separator_spelling_matches_ninja",
        ),
        (
            "DyndepLoadTrivial",
            "dyndep_parser_and_lexer_corpus_matches_ninja_byte_for_byte",
        ),
        (
            "DyndepLoadImplicit",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "DyndepLoadMissingFile",
            "missing_dyndep_diagnostic_matches_ninja",
        ),
        (
            "DyndepLoadMissingEntry",
            "dyndep_file_entry_ownership_diagnostics_match_ninja",
        ),
        (
            "DyndepLoadExtraEntry",
            "dyndep_file_entry_ownership_diagnostics_match_ninja",
        ),
        (
            "DyndepLoadOutputWithMultipleRules1",
            "dyndep_output_conflict_diagnostic_matches_ninja",
        ),
        (
            "DyndepLoadOutputWithMultipleRules2",
            "dyndep_output_conflict_diagnostic_matches_ninja",
        ),
        (
            "DyndepLoadMultiple",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "DyndepFileMissing",
            "missing_dyndep_diagnostic_matches_ninja",
        ),
        (
            "DyndepFileError",
            "graph_loads_only_reachable_dyndeps_and_warns_without_failing",
        ),
        (
            "DyndepImplicitInputNewer",
            "generated_dyndep_keeps_independent_requested_work_concurrent",
        ),
        (
            "DyndepOutputIsDependentInput",
            "ready_dyndep_outputs_are_loaded_before_missing_input_validation",
        ),
        (
            "DyndepOutputIsDependentInputFromDepfile",
            "dependency_log_validations_and_declared_dirty_short_circuit_match_ninja",
        ),
        (
            "DyndepFileReady",
            "ready_dyndep_outputs_are_loaded_before_missing_input_validation",
        ),
        (
            "DyndepFileNotClean",
            "knight_builds_generated_dyndeps_before_dynamic_inputs",
        ),
        (
            "DyndepFileNotReady",
            "knight_builds_generated_dyndeps_before_dynamic_inputs",
        ),
        (
            "DyndepFileSecondNotReady",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "DyndepFileCircular",
            "ready_dyndep_outputs_are_loaded_before_missing_input_validation",
        ),
        (
            "Validation",
            "status_total_excludes_clean_targets_with_dirty_validations",
        ),
        (
            "PhonyDepsMtimes",
            "phony_mtime_ignores_order_only_and_validation_inputs",
        ),
        (
            "EdgeQueuePriority",
            "scheduler_prioritizes_the_longest_remaining_path_like_ninja",
        ),
        (
            "PhonyOutputWithValidation",
            "status_total_excludes_clean_targets_with_dirty_validations",
        ),
    ];
    assert_eq!(cases.len(), 55);
    let names = cases
        .iter()
        .map(|case| case.0)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(names.len(), cases.len());
    assert!(cases.iter().all(|case| !case.1.is_empty()));
}

#[test]
fn upstream_build_core_planning_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let rule = "rule cat\n  command = echo cat $in $out\n";
    let cases = [
        ("no work", "build all: phony\ndefault all\n", &[][..]),
        ("one step", "build out: cat in\ndefault out\n", &["in"][..]),
        (
            "two steps",
            "build mid: cat in\nbuild out: cat mid\ndefault out\n",
            &["in"][..],
        ),
        (
            "two outputs",
            "build out1 out2: cat in\ndefault out1 out2\n",
            &["in"][..],
        ),
        (
            "implicit output",
            "build out | out.imp: cat in\ndefault out\n",
            &["in"][..],
        ),
        (
            "multiple output input",
            "build left right: cat in\nbuild final: cat left right\ndefault final\n",
            &["in"][..],
        ),
        (
            "double dependent",
            "build left: cat in\nbuild right: cat in\nbuild final: cat left right\ndefault final\n",
            &["in"][..],
        ),
        (
            "double output indirect",
            concat!(
                "build a1 a2: cat in\n",
                "build b1: cat a1\n",
                "build b2: cat a2\n",
                "build out: cat b1 b2\n",
                "default out\n",
            ),
            &["in"][..],
        ),
        (
            "multi output implicit input",
            concat!(
                "build in1 otherfile: cat in\n",
                "build out: cat in | in1\n",
                "default out\n",
            ),
            &["in"][..],
        ),
        (
            "encounter ready twice",
            concat!(
                "build c: cat\n",
                "build b: cat || c\n",
                "build a: cat | b || c\n",
                "default a\n",
            ),
            &[][..],
        ),
        (
            "order only",
            "build order: cat order.in\nbuild out: cat in || order\ndefault out\n",
            &["in", "order.in"][..],
        ),
        (
            "phony chain",
            "build alias: phony in\nbuild out: cat alias\ndefault out\n",
            &["in"][..],
        ),
        (
            "complex target",
            "name = foo %2F bar?baz&x=1\nbuild $name: cat in\ndefault $name\n",
            &["in"][..],
        ),
    ];

    for (name, edges, inputs) in cases {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), format!("{rule}{edges}")).unwrap();
        for input in inputs {
            fs::write(temp.path().join(input), "input\n").unwrap();
        }
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        install_ninja_alias(knight, &alias);
        let expected = run(ninja, temp.path(), &["-n", "-v"]);
        let actual = run(&alias, temp.path(), &["-n", "-v"]);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "case={name} actual_stdout={} actual_stderr={} expected_stdout={} expected_stderr={}",
            String::from_utf8_lossy(&actual.stdout),
            String::from_utf8_lossy(&actual.stderr),
            String::from_utf8_lossy(&expected.stdout),
            String::from_utf8_lossy(&expected.stderr),
        );
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }

    for (name, manifest, arguments) in [
        (
            "missing input",
            format!("{rule}build out: cat missing\ndefault out\n"),
            vec!["-n", "-v"],
        ),
        (
            "missing target",
            format!("{rule}build out: cat in\n"),
            vec!["-n", "-v", "absent"],
        ),
        (
            "missing input target",
            format!("{rule}build out: cat missing\n"),
            vec!["-n", "-v", "missing"],
        ),
    ] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), manifest).unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        install_ninja_alias(knight, &alias);
        let expected = run(ninja, temp.path(), &arguments);
        let actual = run(&alias, temp.path(), &arguments);
        assert_eq!(actual.status.code(), expected.status.code(), "case={name}");
        assert_eq!(actual.stdout, expected.stdout, "case={name}");
        assert_eq!(actual.stderr, expected.stderr, "case={name}");
    }
}

#[test]
fn upstream_build_response_file_lifecycle_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    #[cfg(windows)]
    let commands = [
        "cmd /d /c type $rspfile > $out",
        "cmd /d /c type $rspfile & exit /b 1",
    ];
    #[cfg(not(windows))]
    let commands = ["cat $rspfile > $out", "cat $rspfile; false"];

    for (name, command) in [("success", commands[0]), ("failure", commands[1])] {
        let mut outputs = Vec::new();
        for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                format!(
                    concat!(
                        "rule rsp\n",
                        "  command = {command}\n",
                        "  rspfile = args.rsp\n",
                        "  rspfile_content = content\n",
                        "build out: rsp\n",
                        "default out\n",
                    ),
                    command = command
                ),
            )
            .unwrap();
            let alias = temp
                .path()
                .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
            if implementation == "knight" {
                install_ninja_alias(knight, &alias);
            }
            let executable = if implementation == "knight" {
                alias.as_path()
            } else {
                executable
            };
            let output = run(executable, temp.path(), &["-v"]);
            assert_eq!(
                temp.path().join("args.rsp").exists(),
                name == "failure",
                "case={name} implementation={implementation}"
            );
            if name == "failure" {
                assert_eq!(
                    fs::read_to_string(temp.path().join("args.rsp")).unwrap(),
                    "content"
                );
            }
            outputs.push((output.status.code(), output.stdout, output.stderr));
        }
        assert_eq!(outputs[1].0, outputs[0].0, "case={name}");
        assert_eq!(outputs[1].1, outputs[0].1, "case={name}");
        assert_eq!(outputs[1].2, outputs[0].2, "case={name}");
    }
}

#[test]
fn upstream_build_case_inventory_is_complete() {
    const CASES: [&str; 121] = [
        "Basic",
        "DoubleOutputDirect",
        "DoubleOutputIndirect",
        "DoubleDependent",
        "PoolWithDepthOne",
        "ConsolePool",
        "PoolsWithDepthTwo",
        "PoolWithRedundantEdges",
        "PoolWithFailingEdge",
        "PriorityWithoutBuildLog",
        "NoWork",
        "OneStep",
        "OneStep2",
        "TwoStep",
        "TwoOutputs",
        "ImplicitOutput",
        "MultiOutIn",
        "Chain",
        "MissingInput",
        "MissingTarget",
        "MissingInputTarget",
        "MakeDirs",
        "DepFileMissing",
        "DepFileOK",
        "DepFileParseError",
        "EncounterReadyTwice",
        "OrderOnlyDeps",
        "RebuildOrderOnlyDeps",
        "DepFileCanonicalize",
        "Phony",
        "PhonyNoWork",
        "PhonySelfReference",
        "PhonyUseCase1",
        "PhonyUseCase2",
        "PhonyUseCase3",
        "PhonyUseCase4",
        "PhonyUseCase5",
        "PhonyUseCase6",
        "Fail",
        "SwallowFailures",
        "SwallowFailuresLimit",
        "SwallowFailuresPool",
        "PoolEdgesReadyButNotWanted",
        "ImplicitGeneratedOutOfDate",
        "ImplicitGeneratedOutOfDate2",
        "NotInLogButOnDisk",
        "RebuildAfterFailure",
        "RebuildWithNoInputs",
        "RestatTest",
        "RestatMissingFile",
        "RestatSingleDependentOutputDirty",
        "RestatMissingInput",
        "RestatInputChangesDueToRule",
        "GeneratedPlainDepfileMtime",
        "AllCommandsShown",
        "WithDyndep",
        "RspFileSuccess",
        "RspFileFailure",
        "RspFileCmdLineChange",
        "InterruptCleanup",
        "StatFailureAbortsBuild",
        "PhonyWithNoInputs",
        "DepsGccWithEmptyDepfileErrorsOut",
        "StatusFormatElapsed_e",
        "StatusFormatElapsed_w",
        "StatusFormatETA",
        "StatusFormatTimeProgress",
        "StatusFormatReplacePlaceholder",
        "FailedDepsParse",
        "TwoOutputsDepFileMSVC",
        "TwoOutputsDepFileGCCOneLine",
        "TwoOutputsDepFileGCCMultiLineInput",
        "TwoOutputsDepFileGCCMultiLineOutput",
        "TwoOutputsDepFileGCCOnlyMainOutput",
        "TwoOutputsDepFileGCCOnlySecondaryOutput",
        "Straightforward",
        "ObsoleteDeps",
        "DepsIgnoredInDryRun",
        "TestInputMtimeRaceCondition",
        "TestInputMtimeRaceConditionWithDepFile",
        "RestatDepfileDependency",
        "RestatDepfileDependencyDepsLog",
        "DepFileOKDepsLog",
        "DiscoveredDepDuringBuildChanged",
        "DepFileDepsLogCanonicalize",
        "RestatMissingDepfile",
        "RestatMissingDepfileDepslog",
        "WrongOutputInDepfileCausesRebuild",
        "Console",
        "DyndepMissingAndNoRule",
        "DyndepReadyImplicitConnection",
        "DyndepReadySyntaxError",
        "DyndepReadyCircular",
        "DyndepBuild",
        "DyndepBuildSyntaxError",
        "DyndepBuildUnrelatedOutput",
        "DyndepBuildDiscoverNewOutput",
        "DyndepBuildDiscoverNewOutputWithMultipleRules1",
        "DyndepBuildDiscoverNewOutputWithMultipleRules2",
        "DyndepBuildDiscoverNewInput",
        "DyndepBuildDiscoverNewInputWithValidation",
        "DyndepBuildDiscoverNewInputWithTransitiveValidation",
        "DyndepBuildDiscoverImplicitConnection",
        "DyndepBuildDiscoverOutputAndDepfileInput",
        "DyndepBuildDiscoverNowWantEdge",
        "DyndepBuildDiscoverNowWantEdgeAndDependent",
        "DyndepBuildDiscoverCircular",
        "DyndepBuildDiscoverRestat",
        "DyndepBuildDiscoverScheduledEdge",
        "DyndepTwoLevelDirect",
        "DyndepTwoLevelIndirect",
        "DyndepTwoLevelDiscoveredReady",
        "DyndepTwoLevelDiscoveredDirty",
        "DyndepBuildMultiple",
        "Validation",
        "ValidationDependsOnOutput",
        "ValidationThroughDepfile",
        "ValidationCircular",
        "ValidationWithCircularDependency",
        "ComplexTargetPreserved",
        "CycleWithOldDepfile",
    ];
    let unique = CASES.into_iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), CASES.len());

    const EVIDENCE: [(&str, &str); 121] = [
        ("Basic", "upstream_build_core_planning_corpus_matches_ninja"),
        (
            "DoubleOutputDirect",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "DoubleOutputIndirect",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "DoubleDependent",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "PoolWithDepthOne",
            "bounded_pools_reserve_ready_work_like_ninja",
        ),
        (
            "ConsolePool",
            "console_pool_overlaps_work_and_buffers_its_output_like_ninja",
        ),
        (
            "PoolsWithDepthTwo",
            "bounded_pools_reserve_ready_work_like_ninja",
        ),
        (
            "PoolWithRedundantEdges",
            "delayed_pool_work_keeps_its_reservation_before_new_dependents",
        ),
        (
            "PoolWithFailingEdge",
            "upstream_build_failure_limit_corpus_matches_ninja",
        ),
        (
            "PriorityWithoutBuildLog",
            "scheduler_prioritizes_the_longest_remaining_path_like_ninja",
        ),
        (
            "NoWork",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "OneStep",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "OneStep2",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        ("TwoStep", "build::tests::builds_incrementally"),
        (
            "TwoOutputs",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "ImplicitOutput",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "MultiOutIn",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        ("Chain", "build::tests::builds_incrementally"),
        (
            "MissingInput",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "MissingTarget",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "MissingInputTarget",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "MakeDirs",
            "build::tests::upstream_disk_interface_make_dirs_corpus",
        ),
        (
            "DepFileMissing",
            "upstream_build_restat_and_dry_run_corpus_matches_ninja",
        ),
        (
            "DepFileOK",
            "dependency_type_configuration_matches_ninjas_build_phases",
        ),
        ("DepFileParseError", "stale_depfile_failures_match_ninja"),
        (
            "EncounterReadyTwice",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "OrderOnlyDeps",
            "dependency_type_configuration_matches_ninjas_build_phases",
        ),
        (
            "RebuildOrderOnlyDeps",
            "upstream_build_order_only_rebuild_corpus_matches_ninja",
        ),
        (
            "DepFileCanonicalize",
            "upstream_graph_depfile_path_and_removal_cases_match_ninja",
        ),
        (
            "Phony",
            "upstream_build_phony_use_case_corpus_matches_ninja",
        ),
        (
            "PhonyNoWork",
            "upstream_build_phony_use_case_corpus_matches_ninja",
        ),
        (
            "PhonySelfReference",
            "phony_self_reference_policy_matches_ninja_tools",
        ),
        (
            "PhonyUseCase1",
            "upstream_build_phony_use_case_corpus_matches_ninja",
        ),
        (
            "PhonyUseCase2",
            "upstream_build_phony_use_case_corpus_matches_ninja",
        ),
        (
            "PhonyUseCase3",
            "upstream_build_phony_use_case_corpus_matches_ninja",
        ),
        (
            "PhonyUseCase4",
            "upstream_build_phony_use_case_corpus_matches_ninja",
        ),
        (
            "PhonyUseCase5",
            "upstream_build_phony_use_case_corpus_matches_ninja",
        ),
        (
            "PhonyUseCase6",
            "upstream_build_phony_use_case_corpus_matches_ninja",
        ),
        ("Fail", "failed_command_status_and_exit_code_match_ninja"),
        (
            "SwallowFailures",
            "upstream_build_failure_limit_corpus_matches_ninja",
        ),
        (
            "SwallowFailuresLimit",
            "upstream_build_failure_limit_corpus_matches_ninja",
        ),
        (
            "SwallowFailuresPool",
            "upstream_build_failure_limit_corpus_matches_ninja",
        ),
        (
            "PoolEdgesReadyButNotWanted",
            "initial_pool_frontier_includes_clean_phony_dependents",
        ),
        (
            "ImplicitGeneratedOutOfDate",
            "upstream_graph_dirty_state_corpus_matches_ninja",
        ),
        (
            "ImplicitGeneratedOutOfDate2",
            "upstream_build_input_mtime_race_corpus_matches_ninja",
        ),
        (
            "NotInLogButOnDisk",
            "upstream_build_log_and_rsp_command_change_corpus_matches_ninja",
        ),
        (
            "RebuildAfterFailure",
            "upstream_build_log_rebuild_after_failure_and_inputless_corpus_matches_ninja",
        ),
        (
            "RebuildWithNoInputs",
            "upstream_build_log_rebuild_after_failure_and_inputless_corpus_matches_ninja",
        ),
        (
            "RestatTest",
            "unchanged_restat_output_becomes_clean_after_the_command_runs",
        ),
        (
            "RestatMissingFile",
            "missing_to_missing_restat_output_does_not_dirty_its_dependent",
        ),
        (
            "RestatSingleDependentOutputDirty",
            "multi_output_restat_only_cleans_dependents_of_unchanged_outputs",
        ),
        (
            "RestatMissingInput",
            "upstream_build_restat_and_dry_run_corpus_matches_ninja",
        ),
        (
            "RestatInputChangesDueToRule",
            "upstream_build_restat_and_dry_run_corpus_matches_ninja",
        ),
        (
            "GeneratedPlainDepfileMtime",
            "upstream_build_restat_and_dry_run_corpus_matches_ninja",
        ),
        (
            "AllCommandsShown",
            "upstream_build_restat_and_dry_run_corpus_matches_ninja",
        ),
        (
            "WithDyndep",
            "generated_dyndep_dry_run_lists_safe_work_once",
        ),
        (
            "RspFileSuccess",
            "upstream_build_response_file_lifecycle_matches_ninja",
        ),
        (
            "RspFileFailure",
            "upstream_build_response_file_lifecycle_matches_ninja",
        ),
        (
            "RspFileCmdLineChange",
            "upstream_build_log_and_rsp_command_change_corpus_matches_ninja",
        ),
        (
            "InterruptCleanup",
            "interrupt_terminates_the_spawned_process_tree",
        ),
        (
            "StatFailureAbortsBuild",
            "filesystem_stat_failures_abort_before_missing_input_diagnostics",
        ),
        (
            "PhonyWithNoInputs",
            "inputless_phony_target_forces_rebuilds",
        ),
        (
            "DepsGccWithEmptyDepfileErrorsOut",
            "dependency_extraction_failure_is_reported_as_a_buffered_subcommand_failure",
        ),
        (
            "StatusFormatElapsed_e",
            "build::tests::expands_all_ninja_status_placeholders",
        ),
        (
            "StatusFormatElapsed_w",
            "build::tests::expands_all_ninja_status_placeholders",
        ),
        (
            "StatusFormatETA",
            "build::tests::expands_all_ninja_status_placeholders",
        ),
        (
            "StatusFormatTimeProgress",
            "build::tests::expands_all_ninja_status_placeholders",
        ),
        (
            "StatusFormatReplacePlaceholder",
            "build::tests::expands_all_ninja_status_placeholders",
        ),
        (
            "FailedDepsParse",
            "dependency_extraction_failure_is_reported_as_a_buffered_subcommand_failure",
        ),
        (
            "TwoOutputsDepFileMSVC",
            "upstream_build_multi_output_dependency_log_corpus_matches_ninja",
        ),
        (
            "TwoOutputsDepFileGCCOneLine",
            "upstream_build_multi_output_dependency_log_corpus_matches_ninja",
        ),
        (
            "TwoOutputsDepFileGCCMultiLineInput",
            "upstream_build_multi_output_dependency_log_corpus_matches_ninja",
        ),
        (
            "TwoOutputsDepFileGCCMultiLineOutput",
            "upstream_build_multi_output_dependency_log_corpus_matches_ninja",
        ),
        (
            "TwoOutputsDepFileGCCOnlyMainOutput",
            "upstream_build_multi_output_dependency_log_corpus_matches_ninja",
        ),
        (
            "TwoOutputsDepFileGCCOnlySecondaryOutput",
            "upstream_build_multi_output_dependency_log_corpus_matches_ninja",
        ),
        (
            "Straightforward",
            "ninja_and_knight_exchange_gcc_dependency_logs",
        ),
        (
            "ObsoleteDeps",
            "build::tests::accepts_older_outputs_and_first_output_records_from_deps_log",
        ),
        (
            "DepsIgnoredInDryRun",
            "upstream_build_restat_and_dry_run_corpus_matches_ninja",
        ),
        (
            "TestInputMtimeRaceCondition",
            "upstream_build_input_mtime_race_corpus_matches_ninja",
        ),
        (
            "TestInputMtimeRaceConditionWithDepFile",
            "upstream_build_input_mtime_race_corpus_matches_ninja",
        ),
        (
            "RestatDepfileDependency",
            "upstream_build_restat_and_dry_run_corpus_matches_ninja",
        ),
        (
            "RestatDepfileDependencyDepsLog",
            "upstream_build_restat_and_dry_run_corpus_matches_ninja",
        ),
        (
            "DepFileOKDepsLog",
            "ninja_and_knight_exchange_gcc_dependency_logs",
        ),
        (
            "DiscoveredDepDuringBuildChanged",
            "upstream_build_input_mtime_race_corpus_matches_ninja",
        ),
        (
            "DepFileDepsLogCanonicalize",
            "upstream_graph_depfile_path_and_removal_cases_match_ninja",
        ),
        (
            "RestatMissingDepfile",
            "restat_does_not_hide_a_missing_downstream_depfile",
        ),
        (
            "RestatMissingDepfileDepslog",
            "restat_does_not_hide_a_missing_downstream_depfile",
        ),
        (
            "WrongOutputInDepfileCausesRebuild",
            "stale_depfile_failures_match_ninja",
        ),
        (
            "Console",
            "console_pool_inherits_all_terminal_descriptors_like_ninja",
        ),
        (
            "DyndepMissingAndNoRule",
            "missing_dyndep_diagnostic_matches_ninja",
        ),
        (
            "DyndepReadyImplicitConnection",
            "ready_dyndep_outputs_are_loaded_before_missing_input_validation",
        ),
        (
            "DyndepReadySyntaxError",
            "dyndep_parser_and_lexer_corpus_matches_ninja_byte_for_byte",
        ),
        (
            "DyndepReadyCircular",
            "ready_dyndep_outputs_are_loaded_before_missing_input_validation",
        ),
        (
            "DyndepBuild",
            "knight_builds_generated_dyndeps_before_dynamic_inputs",
        ),
        (
            "DyndepBuildSyntaxError",
            "dyndep_parser_and_lexer_corpus_matches_ninja_byte_for_byte",
        ),
        (
            "DyndepBuildUnrelatedOutput",
            "dyndep_file_entry_ownership_diagnostics_match_ninja",
        ),
        (
            "DyndepBuildDiscoverNewOutput",
            "knight_builds_generated_dyndeps_before_dynamic_inputs",
        ),
        (
            "DyndepBuildDiscoverNewOutputWithMultipleRules1",
            "dyndep_output_conflict_diagnostic_matches_ninja",
        ),
        (
            "DyndepBuildDiscoverNewOutputWithMultipleRules2",
            "dyndep_output_conflict_diagnostic_matches_ninja",
        ),
        (
            "DyndepBuildDiscoverNewInput",
            "knight_builds_generated_dyndeps_before_dynamic_inputs",
        ),
        (
            "DyndepBuildDiscoverNewInputWithValidation",
            "dependency_log_validations_and_declared_dirty_short_circuit_match_ninja",
        ),
        (
            "DyndepBuildDiscoverNewInputWithTransitiveValidation",
            "build::tests::collects_transitive_validations_without_creating_false_cycles",
        ),
        (
            "DyndepBuildDiscoverImplicitConnection",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "DyndepBuildDiscoverOutputAndDepfileInput",
            "knight_builds_generated_dyndeps_before_dynamic_inputs",
        ),
        (
            "DyndepBuildDiscoverNowWantEdge",
            "generated_dyndep_keeps_independent_requested_work_concurrent",
        ),
        (
            "DyndepBuildDiscoverNowWantEdgeAndDependent",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "DyndepBuildDiscoverCircular",
            "ready_dyndep_outputs_are_loaded_before_missing_input_validation",
        ),
        (
            "DyndepBuildDiscoverRestat",
            "explain_reports_dyndep_loads_without_duplicate_output_reasons",
        ),
        (
            "DyndepBuildDiscoverScheduledEdge",
            "generated_dyndep_keeps_independent_requested_work_concurrent",
        ),
        (
            "DyndepTwoLevelDirect",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "DyndepTwoLevelIndirect",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "DyndepTwoLevelDiscoveredReady",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "DyndepTwoLevelDiscoveredDirty",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "DyndepBuildMultiple",
            "two_level_dyndep_discovery_reaches_a_fixed_point",
        ),
        (
            "Validation",
            "upstream_build_validation_corpus_matches_ninja",
        ),
        (
            "ValidationDependsOnOutput",
            "upstream_build_validation_corpus_matches_ninja",
        ),
        (
            "ValidationThroughDepfile",
            "dependency_log_validations_and_declared_dirty_short_circuit_match_ninja",
        ),
        (
            "ValidationCircular",
            "upstream_build_validation_corpus_matches_ninja",
        ),
        (
            "ValidationWithCircularDependency",
            "upstream_build_validation_corpus_matches_ninja",
        ),
        (
            "ComplexTargetPreserved",
            "upstream_build_core_planning_corpus_matches_ninja",
        ),
        (
            "CycleWithOldDepfile",
            "stale_depfile_cycle_is_ignored_when_declared_inputs_are_dirty",
        ),
    ];
    assert_eq!(
        EVIDENCE.map(|(case, _)| case),
        CASES,
        "the evidence map must stay aligned with the pinned upstream inventory"
    );
    assert!(EVIDENCE.iter().all(|(_, evidence)| !evidence.is_empty()));
}

#[test]
fn upstream_build_multi_output_dependency_log_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    #[cfg(windows)]
    let copy_command = "cmd /d /c copy /y in1 out1 >nul & copy /y in1 out2 >nul";
    #[cfg(not(windows))]
    let copy_command = "cp in1 out1; cp in1 out2";
    let normalize_mtimes = |output: &[u8]| {
        String::from_utf8_lossy(output)
            .split('\n')
            .map(|line| {
                let mut line = line.to_owned();
                if let Some(marker) = line.find("deps mtime ")
                    && let Some(end) = line[marker..].find(" (")
                {
                    line.replace_range(marker + "deps mtime ".len()..marker + end, "<mtime>");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    for (name, depfile) in [
        ("one line", "out1 out2: in1 in2\n"),
        ("line per input", "out1 out2: in1\nout1 out2: in2\n"),
        ("line per output", "out1: in1 in2\nout2: in1 in2\n"),
        ("main output only", "out1: in1 in2\n"),
        ("secondary output only", "out2: in1 in2\n"),
    ] {
        let mut queries = Vec::new();
        for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                format!(
                    concat!(
                        "rule cc\n",
                        "  command = {copy_command}\n",
                        "  deps = gcc\n",
                        "  depfile = in.d\n",
                        "build out1 out2: cc in1 in2\n",
                        "default out1\n",
                    ),
                    copy_command = copy_command,
                ),
            )
            .unwrap();
            fs::write(temp.path().join("in1"), "one\n").unwrap();
            fs::write(temp.path().join("in2"), "two\n").unwrap();
            fs::write(temp.path().join("in.d"), depfile).unwrap();
            let alias = temp
                .path()
                .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
            if implementation == "knight" {
                install_ninja_alias(knight, &alias);
            }
            let executable = if implementation == "knight" {
                alias.as_path()
            } else {
                executable
            };
            let build = run(executable, temp.path(), &[]);
            assert!(
                build.status.success(),
                "case={name} implementation={implementation} stdout={} stderr={}",
                String::from_utf8_lossy(&build.stdout),
                String::from_utf8_lossy(&build.stderr),
            );
            queries.push(run(
                executable,
                temp.path(),
                &["-t", "deps", "out1", "out2"],
            ));
        }
        assert_eq!(
            queries[1].status.code(),
            queries[0].status.code(),
            "case={name}"
        );
        assert_eq!(
            normalize_mtimes(&queries[1].stdout),
            normalize_mtimes(&queries[0].stdout),
            "case={name}"
        );
        assert_eq!(queries[1].stderr, queries[0].stderr, "case={name}");
    }

    #[cfg(windows)]
    let msvc_command = concat!(
        "cmd /d /c echo using in1&",
        "copy /y in1 out1 >nul&copy /y in1 out2 >nul"
    );
    #[cfg(not(windows))]
    let msvc_command = "printf 'using in1\\n'; cp in1 out1; cp in1 out2";
    let mut queries = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                concat!(
                    "rule cc\n",
                    "  command = {msvc_command}\n",
                    "  deps = msvc\n",
                    "  msvc_deps_prefix = using \n",
                    "build out1 out2: cc in1\n",
                    "default out1\n",
                ),
                msvc_command = msvc_command,
            ),
        )
        .unwrap();
        fs::write(temp.path().join("in1"), "one\n").unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };
        let build = run(executable, temp.path(), &[]);
        assert!(build.status.success(), "implementation={implementation}");
        queries.push(run(
            executable,
            temp.path(),
            &["-t", "deps", "out1", "out2"],
        ));
    }
    assert_eq!(queries[1].status.code(), queries[0].status.code());
    assert_eq!(
        normalize_mtimes(&queries[1].stdout),
        normalize_mtimes(&queries[0].stdout)
    );
    assert_eq!(queries[1].stderr, queries[0].stderr);
}

#[test]
fn upstream_build_log_and_rsp_command_change_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    #[cfg(windows)]
    let rsp_command = "cmd /d /c type $rspfile > $out";
    #[cfg(not(windows))]
    let rsp_command = "cat $rspfile > $out";

    let mut sequences = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        let write_manifest = |content: &str| {
            fs::write(
                temp.path().join("build.ninja"),
                format!(
                    concat!(
                        "rule rsp\n",
                        "  command = {rsp_command}\n",
                        "  rspfile = args.rsp\n",
                        "  rspfile_content = {content}\n",
                        "build out: rsp\n",
                        "default out\n",
                    ),
                    rsp_command = rsp_command,
                    content = content,
                ),
            )
            .unwrap();
        };
        write_manifest("original content");
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };
        let first = run(executable, temp.path(), &["-v"]);
        let second = run(executable, temp.path(), &["-v"]);
        write_manifest("changed content");
        let third = run(executable, temp.path(), &["-v"]);
        assert!(first.status.success() && second.status.success() && third.status.success());
        assert_eq!(
            fs::read_to_string(temp.path().join("out")).unwrap(),
            "changed content"
        );
        sequences.push([
            (first.status.code(), first.stdout, first.stderr),
            (second.status.code(), second.stdout, second.stderr),
            (third.status.code(), third.stdout, third.stderr),
        ]);
    }
    assert_eq!(sequences[1], sequences[0]);

    #[cfg(windows)]
    let write_command = "cmd /d /c echo built>$out";
    #[cfg(not(windows))]
    let write_command = "printf built > $out";
    let mut results = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!("rule write\n  command = {write_command}\nbuild out: write in\ndefault out\n"),
        )
        .unwrap();
        fs::write(temp.path().join("in"), "input\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(temp.path().join("out"), "unlogged output\n").unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };
        results.push(run(executable, temp.path(), &["-v"]));
    }
    assert_eq!(results[1].status.code(), results[0].status.code());
    assert_eq!(results[1].stdout, results[0].stdout);
    assert_eq!(results[1].stderr, results[0].stderr);
}

#[test]
fn upstream_build_failure_limit_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    #[cfg(windows)]
    let fail_command = "cmd /d /c exit 1";
    #[cfg(not(windows))]
    let fail_command = "false";
    let cases = [
        (
            "failure threshold",
            "",
            "build all: phony out1 out2 out3\ndefault all\n",
            "3",
        ),
        (
            "cannot progress",
            "",
            "build final: phony out1 out2 out3\ndefault final\n",
            "11",
        ),
        (
            "cannot progress in pool",
            "pool failpool\n  depth = 1\n",
            "build final: phony out1 out2 out3\ndefault final\n",
            "11",
        ),
    ];

    for (name, pool, tail, limit) in cases {
        let mut outputs = Vec::new();
        for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
            let temp = tempdir().unwrap();
            let pool_binding = if pool.is_empty() {
                ""
            } else {
                "  pool = failpool\n"
            };
            fs::write(
                temp.path().join("build.ninja"),
                format!(
                    concat!(
                        "{pool}",
                        "rule fail\n",
                        "  command = {fail_command}\n",
                        "{pool_binding}",
                        "build out1: fail\n",
                        "build out2: fail\n",
                        "build out3: fail\n",
                        "{tail}",
                    ),
                    pool = pool,
                    fail_command = fail_command,
                    pool_binding = pool_binding,
                    tail = tail,
                ),
            )
            .unwrap();
            let alias = temp
                .path()
                .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
            if implementation == "knight" {
                install_ninja_alias(knight, &alias);
            }
            let executable = if implementation == "knight" {
                alias.as_path()
            } else {
                executable
            };
            outputs.push(run(executable, temp.path(), &["-j1", "-k", limit]));
        }
        assert_eq!(
            outputs[1].status.code(),
            outputs[0].status.code(),
            "case={name}"
        );
        assert_eq!(outputs[1].stdout, outputs[0].stdout, "case={name}");
        assert_eq!(outputs[1].stderr, outputs[0].stderr, "case={name}");
    }
}

#[test]
fn upstream_build_phony_use_case_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    #[cfg(windows)]
    let touch_command = "cmd /d /c type nul > $out";
    #[cfg(not(windows))]
    let touch_command = "touch $out";

    let mut sequences = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                concat!(
                    "rule touch\n",
                    "  command = {touch_command}\n",
                    "build notreal: phony blank\n",
                    "build phony1: phony notreal\n",
                    "build phony2: phony\n",
                    "build phony3: phony blank\n",
                    "build phony4: phony notreal\n",
                    "build phony5: phony\n",
                    "build phony6: phony blank\n",
                    "build test1: touch phony1\n",
                    "build test2: touch phony2\n",
                    "build test3: touch phony3\n",
                    "build test4: touch phony4\n",
                    "build test5: touch phony5\n",
                    "build test6: touch phony6\n",
                    "default test1 test2 test3 test4 test5 test6\n",
                ),
                touch_command = touch_command,
            ),
        )
        .unwrap();
        fs::write(temp.path().join("blank"), "input\n").unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };

        let mut sequence = Vec::new();
        sequence.push(run(executable, temp.path(), &["-j1", "-v"]));
        for case in 1..=6 {
            sequence.push(run(
                executable,
                temp.path(),
                &["-j1", "-v", &format!("test{case}")],
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(temp.path().join("blank"), "changed\n").unwrap();
        for case in 1..=6 {
            sequence.push(run(
                executable,
                temp.path(),
                &["-j1", "-v", &format!("test{case}")],
            ));
        }
        assert!(
            sequence.iter().all(|output| output.status.success()),
            "implementation={implementation}"
        );
        sequences.push(sequence);
    }

    assert_eq!(sequences[1].len(), sequences[0].len());
    for (index, (actual, expected)) in sequences[1].iter().zip(&sequences[0]).enumerate() {
        assert_eq!(actual.status.code(), expected.status.code(), "run={index}");
        assert_eq!(actual.stdout, expected.stdout, "run={index}");
        assert_eq!(actual.stderr, expected.stderr, "run={index}");
    }
}

#[test]
fn upstream_build_input_mtime_race_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    for (name, dependency, depfile) in [
        ("declared input", "in1", false),
        ("discovered input", "header.h", true),
    ] {
        let mut sequences = Vec::new();
        for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
            let temp = tempdir().unwrap();
            #[cfg(windows)]
            let (script_name, command, script) = (
                "race.cmd",
                "race.cmd",
                format!(
                    concat!(
                        "@echo off\r\n",
                        "echo built>out\r\n",
                        "{depfile_line}",
                        "if exist raced.flag exit /b 0\r\n",
                        "powershell -NoProfile -Command \"Start-Sleep -Milliseconds 100\"\r\n",
                        "echo raced>{dependency}\r\n",
                        "echo flag>raced.flag\r\n",
                    ),
                    depfile_line = if depfile {
                        "echo out: header.h>out.d\r\n"
                    } else {
                        ""
                    },
                    dependency = dependency,
                ),
            );
            #[cfg(not(windows))]
            let (script_name, command, script) = (
                "race.sh",
                "sh race.sh",
                format!(
                    concat!(
                        "#!/bin/sh\n",
                        "printf built > out\n",
                        "{depfile_line}",
                        "test -e raced.flag && exit 0\n",
                        "sleep 0.1\n",
                        "printf raced > {dependency}\n",
                        "printf flag > raced.flag\n",
                    ),
                    depfile_line = if depfile {
                        "printf 'out: header.h\\n' > out.d\n"
                    } else {
                        ""
                    },
                    dependency = dependency,
                ),
            );
            fs::write(temp.path().join(script_name), script).unwrap();
            let dependency_binding = if depfile {
                "  deps = gcc\n  depfile = out.d\n"
            } else {
                ""
            };
            let input = if depfile { "" } else { " in1" };
            fs::write(
                temp.path().join("build.ninja"),
                format!(
                    "rule race\n  command = {command}\n{dependency_binding}build out: race{input}\ndefault out\n"
                ),
            )
            .unwrap();
            fs::write(temp.path().join(dependency), "initial\n").unwrap();
            let alias = temp
                .path()
                .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
            if implementation == "knight" {
                install_ninja_alias(knight, &alias);
            }
            let executable = if implementation == "knight" {
                alias.as_path()
            } else {
                executable
            };

            let mut sequence = vec![
                run(executable, temp.path(), &["-v"]),
                run(executable, temp.path(), &["-v"]),
                run(executable, temp.path(), &["-v"]),
            ];
            if depfile {
                std::thread::sleep(std::time::Duration::from_millis(50));
                fs::write(temp.path().join(dependency), "external change\n").unwrap();
                fs::remove_file(temp.path().join("raced.flag")).unwrap();
                sequence.extend([
                    run(executable, temp.path(), &["-v"]),
                    run(executable, temp.path(), &["-v"]),
                    run(executable, temp.path(), &["-v"]),
                ]);
            }
            assert!(
                sequence.iter().all(|output| output.status.success()),
                "case={name} implementation={implementation}"
            );
            sequences.push(sequence);
        }

        assert_eq!(sequences[1].len(), sequences[0].len(), "case={name}");
        for (index, (actual, expected)) in sequences[1].iter().zip(&sequences[0]).enumerate() {
            assert_eq!(
                actual.status.code(),
                expected.status.code(),
                "case={name} run={index}"
            );
            assert_eq!(actual.stdout, expected.stdout, "case={name} run={index}");
            assert_eq!(actual.stderr, expected.stderr, "case={name} run={index}");
        }
    }
}

#[test]
fn upstream_build_order_only_rebuild_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    #[cfg(windows)]
    let copy_command = "cmd /d /c copy /y $in $out >nul";
    #[cfg(not(windows))]
    let copy_command = "cp $in $out";

    let mut sequences = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                concat!(
                    "rule copy\n",
                    "  command = {copy_command}\n",
                    "build oo.h: copy oo.h.in\n",
                    "build out: copy in || oo.h\n",
                    "default out\n",
                ),
                copy_command = copy_command,
            ),
        )
        .unwrap();
        fs::write(temp.path().join("in"), "input\n").unwrap();
        fs::write(temp.path().join("oo.h.in"), "header\n").unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };

        let mut sequence = vec![
            run(executable, temp.path(), &["-j1", "-v"]),
            run(executable, temp.path(), &["-j1", "-v"]),
        ];
        fs::remove_file(temp.path().join("oo.h")).unwrap();
        sequence.push(run(executable, temp.path(), &["-j1", "-v"]));
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(temp.path().join("oo.h.in"), "changed header\n").unwrap();
        sequence.push(run(executable, temp.path(), &["-j1", "-v"]));
        assert!(
            sequence.iter().all(|output| output.status.success()),
            "implementation={implementation}"
        );
        sequences.push(sequence);
    }

    for (index, (actual, expected)) in sequences[1].iter().zip(&sequences[0]).enumerate() {
        assert_eq!(actual.status.code(), expected.status.code(), "run={index}");
        assert_eq!(actual.stdout, expected.stdout, "run={index}");
        assert_eq!(actual.stderr, expected.stderr, "run={index}");
    }
}

#[test]
fn upstream_build_log_rebuild_after_failure_and_inputless_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    let mut failure_sequences = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        #[cfg(windows)]
        let (script_name, command, script) = (
            "build.cmd",
            "build.cmd",
            concat!(
                "@echo off\r\n",
                "echo built>out\r\n",
                "if exist fail.flag exit /b 1\r\n",
            ),
        );
        #[cfg(not(windows))]
        let (script_name, command, script) = (
            "build.sh",
            "sh build.sh",
            concat!(
                "#!/bin/sh\n",
                "printf built > out\n",
                "test ! -e fail.flag\n",
            ),
        );
        fs::write(temp.path().join(script_name), script).unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!("rule build\n  command = {command}\nbuild out: build in\ndefault out\n"),
        )
        .unwrap();
        fs::write(temp.path().join("in"), "input\n").unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };
        let first = run(executable, temp.path(), &["-v"]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(temp.path().join("in"), "changed\n").unwrap();
        fs::write(temp.path().join("fail.flag"), "fail\n").unwrap();
        let second = run(executable, temp.path(), &["-v"]);
        fs::remove_file(temp.path().join("fail.flag")).unwrap();
        let third = run(executable, temp.path(), &["-v"]);
        assert!(first.status.success() && !second.status.success() && third.status.success());
        failure_sequences.push([first, second, third]);
    }
    for (index, (actual, expected)) in failure_sequences[1]
        .iter()
        .zip(&failure_sequences[0])
        .enumerate()
    {
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "failure run={index}"
        );
        assert_eq!(actual.stdout, expected.stdout, "failure run={index}");
        assert_eq!(actual.stderr, expected.stderr, "failure run={index}");
    }

    #[cfg(windows)]
    let write_command = "cmd /d /c echo built>$out";
    #[cfg(not(windows))]
    let write_command = "printf built > $out";
    let mut inputless_sequences = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                concat!(
                    "rule write\n",
                    "  command = {write_command}\n",
                    "build out1: write\n",
                    "build out2: write in\n",
                    "default out1 out2\n",
                ),
                write_command = write_command,
            ),
        )
        .unwrap();
        fs::write(temp.path().join("in"), "input\n").unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };
        let first = run(executable, temp.path(), &["-j1", "-v"]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(temp.path().join("in"), "changed\n").unwrap();
        let second = run(executable, temp.path(), &["-j1", "-v"]);
        let third = run(executable, temp.path(), &["-j1", "-v"]);
        assert!(first.status.success() && second.status.success() && third.status.success());
        inputless_sequences.push([first, second, third]);
    }
    for (index, (actual, expected)) in inputless_sequences[1]
        .iter()
        .zip(&inputless_sequences[0])
        .enumerate()
    {
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "inputless run={index}"
        );
        assert_eq!(actual.stdout, expected.stdout, "inputless run={index}");
        assert_eq!(actual.stderr, expected.stderr, "inputless run={index}");
    }
}

#[test]
fn upstream_build_validation_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    #[cfg(windows)]
    let write_command = "cmd /d /c echo built>$out";
    #[cfg(not(windows))]
    let write_command = "printf built > $out";

    for (name, edges) in [
        (
            "independent validation",
            "build out: write in |@ validate\nbuild validate: write in2\ndefault out\n",
        ),
        (
            "validation depends on output",
            "build out: write in |@ validate\nbuild validate: write in2 | out\ndefault out\n",
        ),
        (
            "circular validations",
            "build out: write in |@ out2\nbuild out2: write in2 |@ out\ndefault out\n",
        ),
    ] {
        let mut sequences = Vec::new();
        for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                format!("rule write\n  command = {write_command}\n{edges}"),
            )
            .unwrap();
            fs::write(temp.path().join("in"), "input\n").unwrap();
            fs::write(temp.path().join("in2"), "validation input\n").unwrap();
            let alias = temp
                .path()
                .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
            if implementation == "knight" {
                install_ninja_alias(knight, &alias);
            }
            let executable = if implementation == "knight" {
                alias.as_path()
            } else {
                executable
            };
            let first = run(executable, temp.path(), &["-j1", "-v"]);
            std::thread::sleep(std::time::Duration::from_millis(50));
            fs::write(temp.path().join("in"), "changed input\n").unwrap();
            let second = run(executable, temp.path(), &["-j1", "-v"]);
            std::thread::sleep(std::time::Duration::from_millis(50));
            fs::write(temp.path().join("in2"), "changed validation\n").unwrap();
            let third = run(executable, temp.path(), &["-j1", "-v"]);
            assert!(
                first.status.success() && second.status.success() && third.status.success(),
                "case={name} implementation={implementation}"
            );
            sequences.push([first, second, third]);
        }
        for (index, (actual, expected)) in sequences[1].iter().zip(&sequences[0]).enumerate() {
            assert_eq!(
                actual.status.code(),
                expected.status.code(),
                "case={name} run={index}"
            );
            assert_eq!(actual.stdout, expected.stdout, "case={name} run={index}");
            assert_eq!(actual.stderr, expected.stderr, "case={name} run={index}");
        }
    }

    let mut cycle_results = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                concat!(
                    "rule write\n",
                    "  command = {write_command}\n",
                    "build out: write in |@ validate\n",
                    "build validate: write validate_in | out\n",
                    "build validate_in: write validate\n",
                    "default out\n",
                ),
                write_command = write_command,
            ),
        )
        .unwrap();
        fs::write(temp.path().join("in"), "input\n").unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };
        cycle_results.push(run(executable, temp.path(), &[]));
    }
    assert_eq!(
        cycle_results[1].status.code(),
        cycle_results[0].status.code()
    );
    assert_eq!(cycle_results[1].stdout, cycle_results[0].stdout);
    assert_eq!(cycle_results[1].stderr, cycle_results[0].stderr);
}

#[test]
fn upstream_build_restat_and_dry_run_corpus_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    #[cfg(windows)]
    let stable_command = "cmd /d /c echo stable>>runs.txt";
    #[cfg(not(windows))]
    let stable_command = "printf 'stable\\n' >> runs.txt";
    #[cfg(windows)]
    let consume_command = "cmd /d /c echo consume>>runs.txt & echo built>$out";
    #[cfg(not(windows))]
    let consume_command = "printf 'consume\\n' >> runs.txt; printf built > $out";

    let mut missing_input_sequences = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                concat!(
                    "rule stable\n",
                    "  command = {stable_command}\n",
                    "  depfile = $out.d\n",
                    "  restat = 1\n",
                    "rule consume\n",
                    "  command = {consume_command}\n",
                    "build out1: stable in\n",
                    "build out2: consume out1\n",
                    "default out2\n",
                ),
                stable_command = stable_command,
                consume_command = consume_command,
            ),
        )
        .unwrap();
        fs::write(temp.path().join("in"), "input\n").unwrap();
        fs::write(
            temp.path().join("out1.d"),
            "out1: will.be.deleted restat.file\n",
        )
        .unwrap();
        fs::write(temp.path().join("will.be.deleted"), "temporary\n").unwrap();
        fs::write(temp.path().join("restat.file"), "stable\n").unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };
        let first = run(executable, temp.path(), &["-j1", "-v"]);
        fs::remove_file(temp.path().join("will.be.deleted")).unwrap();
        let second = run(executable, temp.path(), &["-j1", "-v"]);
        assert!(
            first.status.success() && second.status.success(),
            "{implementation}"
        );
        missing_input_sequences.push([first, second]);
    }
    for (index, (actual, expected)) in missing_input_sequences[1]
        .iter()
        .zip(&missing_input_sequences[0])
        .enumerate()
    {
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "restat run={index}"
        );
        assert_eq!(actual.stdout, expected.stdout, "restat run={index}");
        assert_eq!(actual.stderr, expected.stderr, "restat run={index}");
    }

    #[cfg(windows)]
    let depfile_command = "cmd /d /c echo built>$out & echo out$: inimp>out.d";
    #[cfg(not(windows))]
    let depfile_command = "printf built > $out; printf 'out: inimp\\n' > out.d";
    let mut plain_depfile_sequences = Vec::new();
    for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            format!(
                concat!(
                    "rule generate\n",
                    "  command = {depfile_command}\n",
                    "  depfile = out.d\n",
                    "build out: generate\n",
                    "default out\n",
                ),
                depfile_command = depfile_command,
            ),
        )
        .unwrap();
        fs::write(temp.path().join("inimp"), "implicit\n").unwrap();
        let alias = temp
            .path()
            .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
        if implementation == "knight" {
            install_ninja_alias(knight, &alias);
        }
        let executable = if implementation == "knight" {
            alias.as_path()
        } else {
            executable
        };
        let first = run(executable, temp.path(), &["-v"]);
        let second = run(executable, temp.path(), &["-v"]);
        assert!(
            first.status.success() && second.status.success(),
            "{implementation}"
        );
        plain_depfile_sequences.push([first, second]);
    }
    for (index, (actual, expected)) in plain_depfile_sequences[1]
        .iter()
        .zip(&plain_depfile_sequences[0])
        .enumerate()
    {
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "depfile run={index}"
        );
        assert_eq!(actual.stdout, expected.stdout, "depfile run={index}");
        assert_eq!(actual.stderr, expected.stderr, "depfile run={index}");
    }

    #[cfg(windows)]
    let write_command = "cmd /d /c echo built>$out";
    #[cfg(not(windows))]
    let write_command = "printf built > $out";
    for (name, bindings, edges) in [
        (
            "restat keeps all dry-run commands",
            "  restat = 1\n",
            "build out1: write in\nbuild out2: write out1\nbuild out3: write out2\ndefault out3\n",
        ),
        (
            "deps do not block dry run",
            "  deps = gcc\n  depfile = out.d\n",
            "build out: write in\ndefault out\n",
        ),
    ] {
        let mut outputs = Vec::new();
        for (implementation, executable) in [("ninja", ninja), ("knight", knight)] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                format!("rule write\n  command = {write_command}\n{bindings}{edges}"),
            )
            .unwrap();
            if name.starts_with("restat") {
                for output in ["out1", "out2", "out3"] {
                    fs::write(temp.path().join(output), "old\n").unwrap();
                }
            } else {
                fs::write(temp.path().join("out"), "old\n").unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            fs::write(temp.path().join("in"), "new\n").unwrap();
            let alias = temp
                .path()
                .join(if cfg!(windows) { "ninja.exe" } else { "ninja" });
            if implementation == "knight" {
                install_ninja_alias(knight, &alias);
            }
            let executable = if implementation == "knight" {
                alias.as_path()
            } else {
                executable
            };
            outputs.push(run(executable, temp.path(), &["-n", "-j1", "-v"]));
        }
        assert_eq!(outputs[1].status.code(), outputs[0].status.code(), "{name}");
        assert_eq!(outputs[1].stdout, outputs[0].stdout, "{name}");
        assert_eq!(outputs[1].stderr, outputs[0].stderr, "{name}");
    }
}

#[test]
fn graph_loads_only_reachable_dyndeps_and_warns_without_failing() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule generate\n  command = echo generated\n",
                "build good: generate good.in || good.dd\n  dyndep = good.dd\n",
                "build bad: generate bad.in || bad.dd\n  dyndep = bad.dd\n",
            ),
        )
        .unwrap();
        fs::write(
            temp.path().join("good.dd"),
            "ninja_dyndep_version = 1\nbuild good | dynamic.out: dyndep | dynamic.in\n",
        )
        .unwrap();
        fs::write(temp.path().join("bad.dd"), "malformed\n").unwrap();

        let good = run(executable, temp.path(), &["-t", "graph", "good"]);
        assert!(good.status.success(), "{}", executable.display());
        assert!(good.stderr.is_empty(), "{}", executable.display());
        let good_graph = String::from_utf8_lossy(&good.stdout);
        assert!(good_graph.contains("dynamic.in"));

        let bad = run(executable, temp.path(), &["-t", "graph", "bad"]);
        assert!(bad.status.success(), "{}", executable.display());
        assert!(!bad.stderr.is_empty(), "{}", executable.display());
        let bad_graph = String::from_utf8_lossy(&bad.stdout);
        assert!(bad_graph.contains("bad.in"));
        assert!(!bad_graph.contains("dynamic."));
    }
}

#[test]
fn existing_but_unmentioned_paths_are_not_build_targets() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("build.ninja"), "build out: phony source\n").unwrap();
    fs::write(temp.path().join("source"), "known input").unwrap();
    fs::write(temp.path().join("unmentioned"), "not a graph node").unwrap();

    for arguments in [
        &["source"][..],
        &["unmentioned"][..],
        &["-t", "graph", "source"][..],
        &["-t", "graph", "unmentioned"][..],
    ] {
        let expected = run(ninja, temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "arguments={arguments:?} actual={} expected={}",
            String::from_utf8_lossy(&actual.stderr),
            String::from_utf8_lossy(&expected.stderr)
        );
    }
}

#[test]
fn first_dependent_and_builddir_target_resolution_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "builddir = build\n",
            "build build/source: phony\n",
            "build build/generated: phony build/source\n",
        ),
    )
    .unwrap();

    for arguments in [
        &["build/source^"][..],
        &["source^"][..],
        &["-t", "query", "source^"][..],
        &["-t", "graph", "source^"][..],
    ] {
        let expected = run(ninja, temp.path(), arguments);
        let actual = run(knight, temp.path(), arguments);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "arguments={arguments:?} actual={} expected={}",
            String::from_utf8_lossy(&actual.stderr),
            String::from_utf8_lossy(&expected.stderr)
        );
    }
    let query = run(knight, temp.path(), &["-t", "query", "source^"]);
    assert!(String::from_utf8_lossy(&query.stdout).starts_with("build/generated:"));
}

#[cfg(windows)]
#[test]
fn clean_supports_rules_and_generator_outputs() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule cc\n  command = echo cc\n  rspfile = $out.rsp\n  rspfile_content = x\n",
            "rule configure\n  command = echo configure\n  generator = 1\n",
            "build a.o: cc a.c\n",
            "build b.o: cc b.c\n",
            "build generated.ninja: configure\n",
        ),
    )
    .unwrap();
    for path in ["a.o", "b.o", "a.o.rsp", "b.o.rsp", "generated.ninja"] {
        fs::write(temp.path().join(path), "x").unwrap();
    }

    let dry_arguments = ["-n", "-v", "-t", "clean", "-r", "cc"];
    let dry = run(knight, temp.path(), &dry_arguments);
    assert!(dry.status.success());
    for path in ["a.o", "b.o", "a.o.rsp", "b.o.rsp"] {
        assert!(temp.path().join(path).exists(), "dry-run removed {path}");
    }
    if let Some(ninja) = std::env::var_os("KNIGHT_NINJA") {
        let expected = run(Path::new(&ninja), temp.path(), &dry_arguments);
        assert_eq!(
            String::from_utf8_lossy(&dry.stdout)
                .lines()
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&expected.stdout)
                .lines()
                .collect::<Vec<_>>()
        );
    }

    let rules = run(knight, temp.path(), &["-t", "clean", "-r", "cc"]);
    assert!(rules.status.success());
    assert!(String::from_utf8_lossy(&rules.stdout).contains("4 files"));
    assert!(!temp.path().join("a.o").exists());
    assert!(!temp.path().join("b.o").exists());
    assert!(temp.path().join("generated.ninja").exists());

    let normal = run(knight, temp.path(), &["-t", "clean"]);
    assert!(normal.status.success());
    assert!(temp.path().join("generated.ninja").exists());
    let generators = run(knight, temp.path(), &["-t", "clean", "-g"]);
    assert!(generators.status.success());
    assert!(!temp.path().join("generated.ninja").exists());
}

#[test]
fn clean_loads_dyndeps_and_removes_dynamic_outputs() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let mut executables = vec![knight];
    let ninja;
    if let Some(path) = std::env::var_os("KNIGHT_NINJA") {
        ninja = path;
        executables.push(Path::new(&ninja));
    }
    for executable in executables {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule cc\n  command = unused\n",
                "build out: cc || deps.dd\n  dyndep = deps.dd\n",
            ),
        )
        .unwrap();
        fs::write(
            temp.path().join("deps.dd"),
            "ninja_dyndep_version = 1\nbuild out | out.dynamic: dyndep\n",
        )
        .unwrap();
        fs::write(temp.path().join("out"), "x").unwrap();
        fs::write(temp.path().join("out.dynamic"), "x").unwrap();

        let cleaned = run(executable, temp.path(), &["-t", "clean"]);
        assert!(
            cleaned.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&cleaned.stdout),
            String::from_utf8_lossy(&cleaned.stderr)
        );
        assert!(!temp.path().join("out").exists());
        assert!(!temp.path().join("out.dynamic").exists());
    }
}

#[test]
fn upstream_clean_all_target_rule_and_auxiliary_corpus_matches_ninja() {
    struct Case {
        name: &'static str,
        manifest: &'static str,
        files: &'static [&'static str],
        arguments: &'static [&'static str],
        present_after: &'static [&'static str],
    }

    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let base = concat!(
        "rule cat\n  command = unused\n",
        "rule cat_e\n  command = unused\n",
        "build in1: cat_e src1\n",
        "build out1: cat in1\n",
        "build in2: cat_e src2\n",
        "build out2: cat in2\n",
    );
    let cases = [
        Case {
            name: "CleanAll",
            manifest: base,
            files: &["in1", "out1", "in2", "out2"],
            arguments: &["-t", "clean"],
            present_after: &[],
        },
        Case {
            name: "CleanAllDryRun",
            manifest: base,
            files: &["in1", "out1", "in2", "out2"],
            arguments: &["-n", "-t", "clean"],
            present_after: &["in1", "out1", "in2", "out2"],
        },
        Case {
            name: "CleanTarget",
            manifest: base,
            files: &["in1", "out1", "in2", "out2"],
            arguments: &["-t", "clean", "out1"],
            present_after: &["in2", "out2"],
        },
        Case {
            name: "CleanTargetDryRun",
            manifest: base,
            files: &["in1", "out1", "in2", "out2"],
            arguments: &["-n", "-t", "clean", "out1"],
            present_after: &["in1", "out1", "in2", "out2"],
        },
        Case {
            name: "CleanRule",
            manifest: base,
            files: &["in1", "out1", "in2", "out2"],
            arguments: &["-t", "clean", "-r", "cat_e"],
            present_after: &["out1", "out2"],
        },
        Case {
            name: "CleanRuleDryRun",
            manifest: base,
            files: &["in1", "out1", "in2", "out2"],
            arguments: &["-n", "-t", "clean", "-r", "cat_e"],
            present_after: &["in1", "out1", "in2", "out2"],
        },
        Case {
            name: "CleanTargetMultiOutput",
            manifest: concat!(
                "rule cat\n  command = unused\n",
                "build out1 out2: cat src1\n",
                "build out3: cat src2\n",
            ),
            files: &["out1", "out2", "out3"],
            arguments: &["-t", "clean", "out1"],
            present_after: &["out3"],
        },
        Case {
            name: "CleanRuleGeneratorDefault",
            manifest: concat!(
                "rule cat\n  command = unused\n",
                "rule regen\n  command = unused\n  generator = 1\n",
                "build out1: cat in1\n",
                "build out2: regen in2\n",
            ),
            files: &["out1", "out2"],
            arguments: &["-t", "clean"],
            present_after: &["out2"],
        },
        Case {
            name: "CleanRuleGeneratorIncluded",
            manifest: concat!(
                "rule cat\n  command = unused\n",
                "rule regen\n  command = unused\n  generator = 1\n",
                "build out1: cat in1\n",
                "build out2: regen in2\n",
            ),
            files: &["out1", "out2"],
            arguments: &["-t", "clean", "-g"],
            present_after: &[],
        },
        Case {
            name: "CleanDepFile",
            manifest: concat!(
                "rule cc\n  command = unused\n  depfile = $out.d\n",
                "build out1: cc in1\n",
            ),
            files: &["out1", "out1.d"],
            arguments: &["-t", "clean"],
            present_after: &[],
        },
        Case {
            name: "CleanDepFileOnCleanTarget",
            manifest: concat!(
                "rule cc\n  command = unused\n  depfile = $out.d\n",
                "build out1: cc in1\n",
            ),
            files: &["out1", "out1.d"],
            arguments: &["-t", "clean", "out1"],
            present_after: &[],
        },
        Case {
            name: "CleanDepFileOnCleanRule",
            manifest: concat!(
                "rule cc\n  command = unused\n  depfile = $out.d\n",
                "build out1: cc in1\n",
            ),
            files: &["out1", "out1.d"],
            arguments: &["-t", "clean", "-r", "cc"],
            present_after: &[],
        },
        Case {
            name: "CleanRspFile",
            manifest: concat!(
                "rule cc\n  command = unused\n  rspfile = cc1.rsp\n  rspfile_content = $in\n",
                "build out1: cc in1\n",
            ),
            files: &["out1", "cc1.rsp"],
            arguments: &["-t", "clean"],
            present_after: &[],
        },
        Case {
            name: "CleanRspTarget",
            manifest: concat!(
                "rule cat_rsp\n  command = unused\n  rspfile = $out.rsp\n  rspfile_content = $in\n",
                "build in2: cat_rsp src2\n",
                "build out2: cat_rsp in2\n",
            ),
            files: &["in2", "in2.rsp", "out2", "out2.rsp"],
            arguments: &["-t", "clean", "out2"],
            present_after: &[],
        },
        Case {
            name: "CleanRspRule",
            manifest: concat!(
                "rule cat_rsp\n  command = unused\n  rspfile = $out.rsp\n  rspfile_content = $in\n",
                "build in2: cat_rsp src2\n",
                "build out2: cat_rsp in2\n",
            ),
            files: &["in2", "in2.rsp", "out2", "out2.rsp"],
            arguments: &["-t", "clean", "-r", "cat_rsp"],
            present_after: &[],
        },
        Case {
            name: "CleanPhony",
            manifest: concat!(
                "rule cat\n  command = unused\n",
                "build alias: phony t1 t2\n",
                "build t1: cat\n",
                "build t2: cat\n",
            ),
            files: &["alias", "t1", "t2"],
            arguments: &["-t", "clean", "alias"],
            present_after: &["alias"],
        },
        Case {
            name: "CleanDepFileAndRspFileWithSpaces",
            manifest: concat!(
                "rule cc_dep\n  command = unused\n  depfile = $out.d\n",
                "rule cc_rsp\n  command = unused\n  rspfile = $out.rsp\n  rspfile_content = $in\n",
                "build out$ 1: cc_dep in$ 1\n",
                "build out$ 2: cc_rsp in$ 1\n",
            ),
            files: &["out 1", "out 2", "out 1.d", "out 2.rsp"],
            arguments: &["-t", "clean"],
            present_after: &[],
        },
    ];

    for case in cases {
        let mut expected = None;
        for executable in [Path::new(&ninja), knight] {
            let temp = tempdir().unwrap();
            fs::write(temp.path().join("build.ninja"), case.manifest).unwrap();
            for file in case.files {
                fs::write(temp.path().join(file), []).unwrap();
            }
            let output = run(executable, temp.path(), case.arguments);
            let state = case
                .files
                .iter()
                .map(|path| temp.path().join(path).exists())
                .collect::<Vec<_>>();
            let result = (output.status.code(), output.stdout, output.stderr, state);
            if let Some(expected) = &expected {
                assert_eq!(&result, expected, "{}", case.name);
            } else {
                expected = Some(result);
            }
            for file in case.files {
                assert_eq!(
                    temp.path().join(file).exists(),
                    case.present_after.contains(file),
                    "{}: {file}",
                    case.name
                );
            }
        }
    }
}

#[test]
fn upstream_clean_missing_dyndep_case_matches_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let mut expected = None;
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule cat\n  command = unused\n",
                "build out: cat in || dd\n  dyndep = dd\n",
            ),
        )
        .unwrap();
        fs::write(temp.path().join("out"), []).unwrap();
        fs::write(temp.path().join("out.imp"), []).unwrap();
        let output = run(executable, temp.path(), &["-t", "clean"]);
        let result = (
            output.status.code(),
            output.stdout,
            output.stderr,
            temp.path().join("out").exists(),
            temp.path().join("out.imp").exists(),
        );
        if let Some(expected) = &expected {
            assert_eq!(&result, expected);
        } else {
            expected = Some(result);
        }
        assert!(!temp.path().join("out").exists());
        assert!(temp.path().join("out.imp").exists());
    }
}

#[test]
fn upstream_cleandead_removal_and_repeat_cases_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let mut expected = None;
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            "rule cat\n  command = unused\nbuild current: cat source\n",
        )
        .unwrap();
        fs::write(
            temp.path().join(".ninja_log"),
            "# ninja log v7\n0\t1\t0\tformer\t0\n0\t1\t0\tcurrent\t0\n",
        )
        .unwrap();
        fs::write(temp.path().join("source"), []).unwrap();
        fs::write(temp.path().join("former"), []).unwrap();
        fs::write(temp.path().join("current"), []).unwrap();
        let first = run(executable, temp.path(), &["-t", "cleandead"]);
        let first_state = (
            temp.path().join("former").exists(),
            temp.path().join("current").exists(),
        );
        let second = run(executable, temp.path(), &["-t", "cleandead"]);
        let result = (
            first.status.code(),
            first.stdout,
            first.stderr,
            first_state,
            second.status.code(),
            second.stdout,
            second.stderr,
        );
        if let Some(expected) = &expected {
            assert_eq!(&result, expected);
        } else {
            expected = Some(result);
        }
        assert!(!temp.path().join("former").exists());
        assert!(temp.path().join("current").exists());
    }
}

#[test]
fn clean_ignores_malformed_dyndeps_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            "rule generate\n  command = echo generated\nbuild out: generate in || dd\n  dyndep = dd\n",
        )
        .unwrap();
        fs::write(temp.path().join("dd"), "malformed\n").unwrap();
        fs::write(temp.path().join("out"), "output\n").unwrap();
        let cleaned = run(executable, temp.path(), &["-t", "clean"]);
        assert!(
            cleaned.status.success(),
            "executable={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&cleaned.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&cleaned.stdout).replace('\r', ""),
            "Cleaning... 1 files.\n"
        );
        assert!(cleaned.stderr.is_empty());
        assert!(!temp.path().join("out").exists());
    }
}

#[test]
fn clean_removes_output_directories_and_continues_after_errors_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);

    for nonempty in [false, true] {
        let mut expected = None;
        for executable in [ninja, knight] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                "rule generate\n  command = echo generated\nbuild outdir: generate\nbuild other: generate\n",
            )
            .unwrap();
            fs::create_dir(temp.path().join("outdir")).unwrap();
            if nonempty {
                fs::write(temp.path().join("outdir/child"), "child\n").unwrap();
            }
            fs::write(temp.path().join("other"), "other\n").unwrap();

            let cleaned = run(executable, temp.path(), &["-t", "clean"]);
            let stdout = String::from_utf8_lossy(&cleaned.stdout).replace('\r', "");
            let stderr = String::from_utf8_lossy(&cleaned.stderr)
                .replace('\r', "")
                .replace("ninja:", "tool:")
                .replace("knight:", "tool:");
            let result = (cleaned.status.code(), stdout, stderr);
            if let Some(expected) = &expected {
                assert_eq!(&result, expected, "nonempty={nonempty}");
            } else {
                expected = Some(result);
            }
            assert!(!temp.path().join("other").exists());
            assert_eq!(temp.path().join("outdir").exists(), nonempty);
        }
    }
}

#[test]
fn clean_phony_rule_removes_phony_paths_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let ninja = Path::new(&ninja);
    let mut expected = None;
    for executable in [ninja, knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            "build alias: phony input\n",
        )
        .unwrap();
        fs::write(temp.path().join("alias"), "stale phony path\n").unwrap();
        fs::write(temp.path().join("input"), "input\n").unwrap();
        let cleaned = run(executable, temp.path(), &["-t", "clean", "-r", "phony"]);
        let result = (
            cleaned.status.code(),
            String::from_utf8_lossy(&cleaned.stdout).replace('\r', ""),
            String::from_utf8_lossy(&cleaned.stderr).replace('\r', ""),
        );
        if let Some(expected) = &expected {
            assert_eq!(&result, expected);
        } else {
            expected = Some(result);
        }
        assert!(!temp.path().join("alias").exists());
        assert!(temp.path().join("input").exists());
    }
}

#[test]
fn cleandead_preserves_outputs_that_are_still_inputs_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            "rule generate\n  command = echo generated\nbuild current: generate source | former\n",
        )
        .unwrap();
        fs::write(
            temp.path().join(".ninja_log"),
            "# ninja log v7\n0\t1\t0\tformer\t0\n0\t1\t0\tcurrent\t0\n",
        )
        .unwrap();
        fs::write(temp.path().join("former"), "still live\n").unwrap();
        fs::write(temp.path().join("current"), "current\n").unwrap();

        let cleaned = run(executable, temp.path(), &["-t", "cleandead"]);
        assert!(cleaned.status.success(), "{}", executable.display());
        assert_eq!(
            String::from_utf8_lossy(&cleaned.stdout).replace('\r', ""),
            "Cleaning... 0 files.\n"
        );
        assert!(temp.path().join("former").exists());
        assert!(temp.path().join("current").exists());
    }
}

#[cfg(windows)]
#[test]
fn build_log_read_errors_fail_metadata_tools_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("build.ninja"), "build out: phony\n").unwrap();
        fs::create_dir(temp.path().join(".ninja_log")).unwrap();
        let output = run(executable, temp.path(), &["-n"]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "build {}",
            executable.display()
        );
        assert!(output.stdout.is_empty(), "build {}", executable.display());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("loading build log .ninja_log"),
            "build {}: {}",
            executable.display(),
            String::from_utf8_lossy(&output.stderr)
        );

        for tool in [
            "query",
            "deps",
            "missingdeps",
            "cleandead",
            "restat",
            "recompact",
        ] {
            let temp = tempdir().unwrap();
            fs::write(temp.path().join("build.ninja"), "build out: phony\n").unwrap();
            fs::create_dir(temp.path().join(".ninja_log")).unwrap();
            let output = run(executable, temp.path(), &["-t", tool]);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{tool} {}",
                executable.display()
            );
            assert!(output.stdout.is_empty(), "{tool} {}", executable.display());
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("loading build log .ninja_log"),
                "{tool} {}: {}",
                executable.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn build_log_directories_follow_ninjas_posix_behavior() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for arguments in [
        &["-n"][..],
        &["-t", "cleandead"][..],
        &["-t", "restat"][..],
        &["-t", "recompact"][..],
    ] {
        let mut expected = None;
        for executable in [Path::new(&ninja), knight] {
            let temp = tempdir().unwrap();
            fs::write(temp.path().join("build.ninja"), "build out: phony\n").unwrap();
            fs::create_dir(temp.path().join(".ninja_log")).unwrap();
            let output = run(executable, temp.path(), arguments);
            let result = (
                output.status.code(),
                String::from_utf8_lossy(&output.stdout)
                    .replace('\r', "")
                    .replace("ninja:", "tool:")
                    .replace("knight:", "tool:"),
                output.stderr.is_empty(),
            );
            if let Some(expected) = &expected {
                assert_eq!(&result, expected, "{arguments:?}");
            } else {
                expected = Some(result);
            }
        }
    }
}

#[test]
fn build_log_versions_match_ninja_before_log_aware_work() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for arguments in [
        &["-n"][..],
        &["-t", "query", "out"][..],
        &["-t", "deps"][..],
        &["-t", "missingdeps", "out"][..],
        &["-t", "cleandead"][..],
        &["-t", "recompact"][..],
    ] {
        for (contents, valid) in [
            ("invalid\n", false),
            ("# ninja log v6\n", false),
            ("# ninja log v8\n", false),
            ("# ninja log v+7 trailing\n", true),
            ("# ninja log v7", true),
        ] {
            let mut expected = None;
            for executable in [Path::new(&ninja), knight] {
                let temp = tempdir().unwrap();
                fs::write(
                    temp.path().join("build.ninja"),
                    "build out: phony\ndefault out\n",
                )
                .unwrap();
                let log_path = temp.path().join(".ninja_log");
                fs::write(&log_path, contents).unwrap();
                let output = run(executable, temp.path(), arguments);
                let result = (
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout)
                        .replace('\r', "")
                        .replace("ninja:", "tool:")
                        .replace("knight:", "tool:"),
                    String::from_utf8_lossy(&output.stderr)
                        .replace('\r', "")
                        .replace("ninja:", "tool:")
                        .replace("knight:", "tool:"),
                );
                if let Some(expected) = &expected {
                    assert_eq!(&result, expected, "{arguments:?}, {contents:?}");
                } else {
                    expected = Some(result);
                }
                assert_eq!(log_path.exists(), valid, "{arguments:?}, {contents:?}");
            }
        }
    }
}

#[test]
fn deps_log_errors_and_tool_loading_phases_match_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        #[cfg(windows)]
        for arguments in [
            &["-n"][..],
            &["-t", "query", "out"][..],
            &["-t", "deps"][..],
            &["-t", "missingdeps", "out"][..],
            &["-t", "cleandead"][..],
            &["-t", "recompact"][..],
        ] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                "build out: phony input\ndefault out\n",
            )
            .unwrap();
            fs::create_dir(temp.path().join(".ninja_deps")).unwrap();
            let output = run(executable, temp.path(), arguments);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{arguments:?} {}",
                executable.display()
            );
            assert!(
                output.stdout.is_empty(),
                "{arguments:?} {}",
                executable.display()
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("loading deps log .ninja_deps"),
                "{arguments:?} {}: {}",
                executable.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        for arguments in [
            &["-t", "commands", "out"][..],
            &["-t", "inputs", "out"][..],
            &["-t", "graph", "out"][..],
            &["-t", "compdb-targets", "out"][..],
        ] {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("build.ninja"),
                "build out: phony input\ndefault out\n",
            )
            .unwrap();
            fs::create_dir(temp.path().join(".ninja_deps")).unwrap();
            let output = run(executable, temp.path(), arguments);
            assert!(
                output.status.success(),
                "{arguments:?} {}: {}",
                executable.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            "build out: phony\ndefault out\n",
        )
        .unwrap();
        fs::write(temp.path().join(".ninja_deps"), "invalid\n").unwrap();
        let output = run(executable, temp.path(), &["-n"]);
        assert!(output.status.success(), "{}", executable.display());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("warning: bad deps log signature or version; starting over"),
            "{}: {}",
            executable.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!temp.path().join(".ninja_deps").exists());
    }
}

#[cfg(windows)]
#[test]
fn console_pool_overlaps_work_and_buffers_its_output_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule foreground\n",
                "  command = powershell -NoProfile -Command \"Set-Content console-start ready; ",
                "for ($$i=0; $$i -lt 100 -and -not (Test-Path normal-done); $$i++) ",
                "{ Start-Sleep -Milliseconds 20 }; if (-not (Test-Path normal-done)) { exit 9 }; ",
                "Write-Output console-output; Set-Content $out done\"\n",
                "  description = CONSOLE\n",
                "  pool = console\n",
                "rule background\n",
                "  command = powershell -NoProfile -Command \"for ($$i=0; $$i -lt 100 ",
                "-and -not (Test-Path console-start); $$i++) { Start-Sleep -Milliseconds 20 }; ",
                "if (-not (Test-Path console-start)) { exit 8 }; Set-Content normal-done ready; ",
                "Write-Output normal-output; Set-Content $out done\"\n",
                "  description = NORMAL\n",
                "build foreground.out: foreground\n",
                "build background.out: background\n",
                "build all: phony foreground.out background.out\n",
                "default all\n",
            ),
        )
        .unwrap();
        let built = run(executable, temp.path(), &["-j", "2"]);
        assert!(
            built.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&built.stdout),
            String::from_utf8_lossy(&built.stderr)
        );
        let stdout = String::from_utf8_lossy(&built.stdout);
        let status_position = stdout.find("[0/2] CONSOLE").unwrap();
        let console_output = stdout.find("console-output").unwrap();
        let normal_output = stdout.find("normal-output").unwrap();
        assert!(
            status_position < console_output && console_output < normal_output,
            "{stdout}"
        );
    }
}

#[cfg(unix)]
#[test]
fn console_pool_overlaps_posix_work_and_buffers_its_output_like_ninja() {
    let Some(ninja) = std::env::var_os("KNIGHT_NINJA") else {
        eprintln!("skipped: set KNIGHT_NINJA to run differential tests");
        return;
    };
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    for executable in [Path::new(&ninja), knight] {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            concat!(
                "rule foreground\n",
                "  command = touch console-start; for i in $$(seq 1 100); do ",
                "[ -e normal-done ] && break; sleep 0.02; done; ",
                "[ -e normal-done ] || exit 9; echo console-output; touch $out\n",
                "  description = CONSOLE\n",
                "  pool = console\n",
                "rule background\n",
                "  command = for i in $$(seq 1 100); do [ -e console-start ] && break; ",
                "sleep 0.02; done; [ -e console-start ] || exit 8; touch normal-done; ",
                "echo normal-output; touch $out\n",
                "  description = NORMAL\n",
                "build foreground.out: foreground\n",
                "build background.out: background\n",
                "build all: phony foreground.out background.out\n",
                "default all\n",
            ),
        )
        .unwrap();
        let built = run(executable, temp.path(), &["-j", "2"]);
        assert!(
            built.status.success(),
            "executable={} stdout={} stderr={}",
            executable.display(),
            String::from_utf8_lossy(&built.stdout),
            String::from_utf8_lossy(&built.stderr)
        );
        let stdout = String::from_utf8_lossy(&built.stdout);
        let status_position = stdout.find("[0/2] CONSOLE").unwrap();
        let console_output = stdout.find("console-output").unwrap();
        let normal_output = stdout.find("normal-output").unwrap();
        assert!(
            status_position < console_output && console_output < normal_output,
            "{stdout}"
        );
    }
}

#[cfg(windows)]
#[test]
fn inherited_jobserver_limits_parallel_commands() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule work\n",
            "  command = powershell -NoProfile -Command \"Add-Content trace start-$out; ",
            "Start-Sleep -Milliseconds 150; Add-Content trace end-$out; Set-Content $out done\"\n",
            "build one: work\n",
            "build two: work\n",
            "build three: work\n",
            "build all: phony one two three\n",
            "default all\n",
        ),
    )
    .unwrap();
    let jobserver = jobserver::Client::new(0).unwrap();
    let mut command = Command::new(knight);
    command.current_dir(temp.path());
    jobserver.configure_make(&mut command);
    let built = command.output().unwrap();
    assert!(
        built.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );
    let trace = fs::read_to_string(temp.path().join("trace")).unwrap();
    let mut active = 0usize;
    for line in trace.lines() {
        if line.starts_with("start-") {
            active += 1;
            assert_eq!(active, 1, "jobserver allowed concurrent work:\n{trace}");
        } else if line.starts_with("end-") {
            active -= 1;
        }
    }
    assert_eq!(active, 0);
}

#[cfg(unix)]
#[test]
fn inherited_pipe_jobserver_limits_parallel_commands() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule work\n",
            "  command = printf 'start-$out\\n' >> trace; sleep 0.1; ",
            "printf 'end-$out\\n' >> trace; touch $out\n",
            "build one: work\n",
            "build two: work\n",
            "build three: work\n",
            "build all: phony one two three\n",
            "default all\n",
        ),
    )
    .unwrap();
    let jobserver = jobserver::Client::new(0).unwrap();
    let mut command = Command::new(knight);
    command.current_dir(temp.path());
    jobserver.configure_make(&mut command);
    let built = command.output().unwrap();
    assert!(
        built.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );
    let trace = fs::read_to_string(temp.path().join("trace")).unwrap();
    let mut active = 0usize;
    for line in trace.lines() {
        if line.starts_with("start-") {
            active += 1;
            assert_eq!(
                active, 1,
                "pipe jobserver allowed concurrent work:\n{trace}"
            );
        } else if line.starts_with("end-") {
            active -= 1;
        }
    }
    assert_eq!(active, 0);
}

#[cfg(unix)]
#[test]
fn returned_jobserver_token_wakes_a_waiting_build_client() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let root = tempdir().unwrap();
    let short = root.path().join("short");
    let long = root.path().join("long");
    fs::create_dir_all(&short).unwrap();
    fs::create_dir_all(&long).unwrap();
    let manifest = |duration: &str| {
        format!(
            concat!(
                "rule work\n",
                "  command = python3 -c 'import time; print(time.time_ns())' > $out; sleep {duration}; python3 -c 'import time; print(time.time_ns())' >> $out\n",
                "build out0: work\n",
                "build out1: work\n",
                "build all: phony out0 out1\n",
                "default all\n",
            ),
            duration = duration,
        )
    };
    fs::write(short.join("build.ninja"), manifest("0.3")).unwrap();
    fs::write(long.join("build.ninja"), manifest("0.8")).unwrap();

    let jobserver = jobserver::Client::new(1).unwrap();
    let mut short_command = Command::new(knight);
    short_command.current_dir(&short).arg("--quiet");
    jobserver.configure_make(&mut short_command);
    let mut short_child = short_command.spawn().unwrap();
    for _ in 0..200 {
        if short.join("out0").exists() && short.join("out1").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(short.join("out0").exists() && short.join("out1").exists());

    let mut long_command = Command::new(knight);
    long_command.current_dir(&long).arg("--quiet");
    jobserver.configure_make(&mut long_command);
    let mut long_child = long_command.spawn().unwrap();
    assert!(short_child.wait().unwrap().success());
    assert!(long_child.wait().unwrap().success());

    let spans = ["out0", "out1"].map(|output| {
        let values = fs::read_to_string(long.join(output))
            .unwrap()
            .lines()
            .map(|line| line.parse::<u128>().unwrap())
            .collect::<Vec<_>>();
        (values[0], values[1])
    });
    let later_start = spans.iter().map(|span| span.0).max().unwrap();
    let earlier_end = spans.iter().map(|span| span.1).min().unwrap();
    assert!(
        later_start < earlier_end,
        "returned token did not wake the second long command: {spans:?}"
    );
}

#[cfg(windows)]
#[test]
fn inherited_jobserver_is_forwarded_to_child_build_tools() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule probe\n",
            "  command = powershell -NoProfile -Command \"Set-Content $out $$env:MAKEFLAGS\"\n",
            "build flags: probe\n",
            "default flags\n",
        ),
    )
    .unwrap();
    let jobserver = jobserver::Client::new(1).unwrap();
    let mut command = Command::new(knight);
    command.current_dir(temp.path());
    jobserver.configure_make(&mut command);
    let built = command.output().unwrap();
    assert!(
        built.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );
    let inherited = fs::read_to_string(temp.path().join("flags")).unwrap();
    assert!(inherited.contains("--jobserver-auth="), "{inherited}");
}

#[cfg(windows)]
#[test]
fn inherited_make_dry_run_flag_suppresses_commands() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "rule write\n  command = cmd /d /c echo built>$out\nbuild out: write\ndefault out\n",
    )
    .unwrap();
    let output = Command::new(knight)
        .current_dir(temp.path())
        .env("MAKEFLAGS", "n")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!temp.path().join("out").exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("echo built>out"));
}

#[cfg(windows)]
#[test]
fn interrupt_terminates_the_spawned_process_tree() {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};
    use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule wait\n",
            "  command = powershell -NoProfile -Command \"Set-Content out partial; ",
            "Set-Content out.d partial; $$p = Start-Process powershell ",
            "-ArgumentList '-NoProfile','-Command','Start-Sleep -Milliseconds 1800; ",
            "Set-Content sentinel child' -PassThru; Set-Content started $$p.Id; ",
            "Start-Sleep -Seconds 10\"\n",
            "  depfile = out.d\n",
            "build out: wait\ndefault out\n",
        ),
    )
    .unwrap();
    let mut child = Command::new(knight)
        .current_dir(temp.path())
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if temp.path().join("started").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(temp.path().join("started").exists());
    // SAFETY: `child` was created as a distinct console process group whose
    // identifier is its process id.
    assert_ne!(
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) },
        0
    );
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(2));
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    assert!(
        !temp.path().join("sentinel").exists(),
        "descendant survived the interrupted build"
    );
    assert!(!temp.path().join("out").exists());
    assert!(!temp.path().join("out.d").exists());

    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule wait\n",
            "  command = powershell -NoProfile -Command \"Set-Content started2 ready; ",
            "Start-Sleep -Seconds 10\"\n",
            "build out: wait input\ndefault out\n",
        ),
    )
    .unwrap();
    fs::write(temp.path().join("out"), "retained").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(temp.path().join("input"), "newer").unwrap();
    let mut child = Command::new(knight)
        .current_dir(temp.path())
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if temp.path().join("started2").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(temp.path().join("started2").exists());
    assert_ne!(
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) },
        0
    );
    assert_eq!(child.wait().unwrap().code(), Some(2));
    assert_eq!(
        fs::read_to_string(temp.path().join("out")).unwrap(),
        "retained"
    );
}

#[cfg(unix)]
#[test]
fn child_exit_130_is_an_interrupted_build_like_ninja() {
    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        "rule stop\n  command = exit 130\nbuild out: stop\ndefault out\n",
    )
    .unwrap();
    let output = run(knight, temp.path(), &[]);
    assert_eq!(output.status.code(), Some(130));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "knight: build stopped: interrupted by user.\n"
    );
}

#[cfg(unix)]
#[test]
fn interrupt_terminates_the_spawned_process_group() {
    use std::os::unix::process::ExitStatusExt;

    let knight = Path::new(env!("CARGO_BIN_EXE_knight"));
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule wait\n",
            "  command = echo partial > out; echo partial > out.d; ",
            "(sleep 2; echo child > sentinel) & echo $$! > started; sleep 10\n",
            "  depfile = out.d\n",
            "build out: wait\ndefault out\n",
        ),
    )
    .unwrap();
    let mut child = Command::new(knight)
        .current_dir(temp.path())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if temp.path().join("started").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(temp.path().join("started").exists());
    // SAFETY: `child.id()` names the live Knight process started above.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130), "signal={:?}", status.signal());
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    assert!(!temp.path().join("sentinel").exists());
    assert!(!temp.path().join("out").exists());
    assert!(!temp.path().join("out.d").exists());

    fs::write(
        temp.path().join("build.ninja"),
        concat!(
            "rule wait\n",
            "  command = echo $$ > started2; sleep 10\n",
            "build out: wait input\ndefault out\n",
        ),
    )
    .unwrap();
    fs::write(temp.path().join("out"), "retained").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(temp.path().join("input"), "newer").unwrap();
    let mut child = Command::new(knight)
        .current_dir(temp.path())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if temp.path().join("started2").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(temp.path().join("started2").exists());
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    assert_eq!(child.wait().unwrap().code(), Some(130));
    assert_eq!(
        fs::read_to_string(temp.path().join("out")).unwrap(),
        "retained"
    );
}
