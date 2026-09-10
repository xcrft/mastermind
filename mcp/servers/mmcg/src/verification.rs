//! Interpret reported outcomes without executing commands or expanding shell syntax.

use crate::executor_report::VerifyResult;

pub(crate) enum PassContradiction {
    NonZeroExit(i32),
    ZeroTests,
}

pub(crate) fn pass_contradiction(row: &VerifyResult) -> Option<PassContradiction> {
    let observed = row.observed.as_ref()?;
    if let Some(code) = observed.exit_code.filter(|code| *code != 0) {
        return Some(PassContradiction::NonZeroExit(code));
    }
    if observed.tests_run == Some(0) && test_runner(&row.cmd).is_some() {
        return Some(PassContradiction::ZeroTests);
    }
    None
}

pub(crate) enum TestRunner {
    Cargo,
    Go,
    Pytest,
    Javascript,
}

/// Recognize a small subset of direct, finite test invocations. Non-test modes
/// and unknown syntax both return None: neither implies a positive test count.
/// This describes command intent, not execution or implicit configuration.
pub(crate) fn test_runner(cmd: &str) -> Option<TestRunner> {
    let cmd = cmd.trim();
    if !cmd
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b" \t-_./:=,+".contains(&c))
    {
        return None;
    }
    let tokens: Vec<_> = cmd.split_ascii_whitespace().collect();
    let (runner, args, switches, values): (_, _, &[_], &[_]) = match tokens.as_slice() {
        ["cargo" | "cargo.exe", "test", args @ ..] => (
            TestRunner::Cargo,
            args,
            &[
                "--lib",
                "--bins",
                "--tests",
                "--doc",
                "--workspace",
                "--all",
                "--all-features",
                "--no-default-features",
                "--release",
                "-r",
                "--locked",
                "--offline",
                "--frozen",
                "--quiet",
                "-q",
                "--verbose",
                "-v",
                "-vv",
                "--no-fail-fast",
            ],
            &[
                "--manifest-path",
                "--package",
                "-p",
                "--exclude",
                "--test",
                "--bin",
                "--features",
                "-F",
                "--target",
                "--profile",
                "--target-dir",
            ],
        ),
        ["go" | "go.exe", "test", args @ ..] => (
            TestRunner::Go,
            args,
            &["-v", "-race", "-short", "-failfast", "-cover", "-json"],
            &["-run", "-count"],
        ),
        ["pytest" | "pytest.exe", args @ ..]
        | ["python" | "python.exe" | "python3" | "python3.exe", "-m", "pytest", args @ ..] => (
            TestRunner::Pytest,
            args,
            &[
                "-q",
                "-qq",
                "-v",
                "-vv",
                "-x",
                "--exitfirst",
                "--disable-warnings",
                "--strict-markers",
                "--strict-config",
                "--setup-show",
            ],
            &["-k", "-m"],
        ),
        ["jest" | "jest.exe", args @ ..] => (
            TestRunner::Javascript,
            args,
            &[
                "--ci",
                "--runInBand",
                "--coverage",
                "--verbose",
                "--silent",
                "--no-cache",
                "--detectOpenHandles",
                "--forceExit",
            ],
            &["-t", "--testNamePattern"],
        ),
        ["vitest" | "vitest.exe", "run" | "--run", args @ ..] => (
            TestRunner::Javascript,
            args,
            &["--coverage", "--silent"],
            &["-t", "--testNamePattern"],
        ),
        _ => return None,
    };
    // With --run, a bare positional can select a Vitest subcommand (e.g. list)
    // before it becomes a filter. Explicit `vitest run` has no such ambiguity.
    let allow_filters = !matches!(tokens.as_slice(), ["vitest" | "vitest.exe", "--run", ..]);
    supported_args(args, switches, values, allow_filters).then_some(runner)
}

fn supported_args(args: &[&str], switches: &[&str], values: &[&str], allow_filters: bool) -> bool {
    let mut args = args.iter().copied();
    while let Some(arg) = args.next() {
        if !arg.starts_with('-') {
            if !allow_filters {
                return false;
            }
            continue;
        }
        if switches.contains(&arg) {
            continue;
        }
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg, None), |(flag, value)| (flag, Some(value)));
        if !values.contains(&flag) {
            return false;
        }
        let Some(value) = inline.or_else(|| args.next()) else {
            return false;
        };
        if value.is_empty() || value.starts_with('-') {
            return false;
        }
        if flag == "-count" && !value.parse::<u32>().is_ok_and(|n| n > 0) {
            return false;
        }
    }
    true
}
