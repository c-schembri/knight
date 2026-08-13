use crate::depfile::parse_depfile;
use crate::deps_log::{DepsLog, deps_log_path};
use crate::dyndep::{DyndepRecord, parse_dyndep};
use crate::manifest::{
    Edge, Manifest, canonicalize_owned_path, canonicalize_path, unknown_target_message,
};
use crate::program_name;
use rapidhash::fast::{RapidHashMap as HashMap, RapidHashSet as HashSet};
use rapidhash::v1::rapidhash_v1 as hash;
use rapidhash::{HashMapExt, HashSetExt};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, VecDeque};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::process::{Command, Output};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

macro_rules! eprintln {
    ($($argument:tt)*) => {{
        write_build_stderr(format_args!($($argument)*));
    }};
}

fn write_build_stderr(arguments: std::fmt::Arguments<'_>) {
    let mut text = arguments.to_string();
    text.push('\n');
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    write_build_text(&mut stderr, text.as_bytes()).expect("failed printing to stderr");
}

thread_local! {
    static LAST_BUILD_EXIT_CODE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

pub fn last_build_exit_code() -> Option<u8> {
    LAST_BUILD_EXIT_CODE.with(|code| (code.get() != 0).then(|| code.get()))
}

#[cfg(windows)]
static PROCESS_JOB: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[cfg(unix)]
static ACTIVE_PROCESS_GROUPS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::BTreeSet<u32>>,
> = std::sync::OnceLock::new();

#[derive(Debug)]
struct ActiveCleanup {
    outputs: Vec<(PathBuf, Option<u128>)>,
    depfile: Option<PathBuf>,
}

static ACTIVE_CLEANUPS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, ActiveCleanup>>,
> = std::sync::OnceLock::new();

fn register_active_cleanup(
    edge: usize,
    outputs: Vec<(PathBuf, Option<u128>)>,
    depfile: Option<PathBuf>,
) {
    ACTIVE_CLEANUPS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(edge, ActiveCleanup { outputs, depfile });
}

fn unregister_active_cleanup(edge: usize) {
    if let Some(active) = ACTIVE_CLEANUPS.get() {
        active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&edge);
    }
}

fn cleanup_interrupted_outputs() {
    let Some(active) = ACTIVE_CLEANUPS.get() else {
        return;
    };
    let mut active = active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for cleanup in active.values() {
        for (output, previous_mtime) in &cleanup.outputs {
            if cleanup.depfile.is_some() || modified_ns(output) != *previous_mtime {
                let _ = fs::remove_file(output);
            }
        }
        if let Some(depfile) = &cleanup.depfile {
            let _ = fs::remove_file(depfile);
        }
    }
    active.clear();
}

#[cfg(windows)]
pub fn install_interrupt_handler() -> Result<(), String> {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    use windows_sys::Win32::System::Threading::ExitProcess;

    static INSTALLED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    unsafe extern "system" fn handler(event: u32) -> i32 {
        if event != CTRL_C_EVENT && event != CTRL_BREAK_EVENT {
            return 0;
        }
        let job = PROCESS_JOB.load(std::sync::atomic::Ordering::Relaxed) as *mut std::ffi::c_void;
        // The current process belongs to this job, so TerminateJobObject may
        // end the callback before it returns. Cleanup must happen first.
        cleanup_interrupted_outputs();
        if !job.is_null() {
            // SAFETY: the process-lifetime handle is published only after the
            // job is fully configured and remains open until process exit.
            unsafe { TerminateJobObject(job, 2) };
        }
        // SAFETY: immediate termination is required in a console callback and
        // closes the job handle, which tears down all descendant processes.
        unsafe { ExitProcess(2) }
    }
    INSTALLED
        .get_or_init(|| {
            // SAFETY: handler has the required ABI and uses only lock-free/API calls.
            if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
                Err(format!(
                    "installing interrupt handler: {}",
                    io::Error::last_os_error()
                ))
            } else {
                Ok(())
            }
        })
        .clone()
}

#[cfg(unix)]
pub fn install_interrupt_handler() -> Result<(), String> {
    ctrlc::set_handler(|| {
        terminate_active_process_groups();
        cleanup_interrupted_outputs();
        // Ninja reports every handled POSIX termination signal as 128 + SIGINT.
        std::process::exit(130);
    })
    .map_err(|error| format!("installing interrupt handler: {error}"))
}

#[cfg(unix)]
fn terminate_active_process_groups() {
    if let Some(groups) = ACTIVE_PROCESS_GROUPS.get()
        && let Ok(groups) = groups.try_lock()
    {
        for group in groups.iter() {
            // SAFETY: each id is recorded immediately after spawning a child
            // whose process group id equals its process id.
            unsafe { libc::kill(-(*group as i32), libc::SIGTERM) };
        }
    }
}

#[cfg(not(any(windows, unix)))]
pub fn install_interrupt_handler() -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
pub fn ensure_process_tree_cleanup() -> Result<(), String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    static CONFIGURED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    CONFIGURED
        .get_or_init(|| {
            // SAFETY: null attributes/name create a private job object.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err(format!(
                    "creating process job: {}",
                    io::Error::last_os_error()
                ));
            }
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `limits` matches the selected information class.
            let configured = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            };
            // SAFETY: the current-process pseudo-handle is valid for assignment.
            let assigned = configured != 0
                && unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) } != 0;
            if !assigned {
                let error = io::Error::last_os_error();
                // SAFETY: assignment failed, so closing this private job cannot
                // terminate the current process and the handle is owned here.
                unsafe { CloseHandle(job) };
                return Err(format!("configuring process-tree cleanup: {error}"));
            }
            PROCESS_JOB.store(job as isize, std::sync::atomic::Ordering::Release);
            Ok(())
        })
        .clone()
}

#[cfg(not(windows))]
pub fn ensure_process_tree_cleanup() -> Result<(), String> {
    Ok(())
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub jobs: usize,
    pub failures_allowed: usize,
    pub dry_run: bool,
    pub verbose: bool,
    pub explain: bool,
    pub stats: bool,
    pub quiet_no_work: bool,
    pub quiet: bool,
    pub status_format: String,
    pub status_format_explicit: bool,
    pub keep_depfile: bool,
    pub keep_rsp: bool,
    pub use_stat_cache: bool,
    pub phony_cycle_error: bool,
    pub max_load_average: f64,
    pub jobserver: Option<jobserver::Client>,
    pub use_jobserver: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        let processors = thread::available_parallelism().map_or(1, usize::from);
        Self {
            jobs: guess_parallelism(processors),
            failures_allowed: 1,
            dry_run: false,
            verbose: false,
            explain: false,
            stats: false,
            quiet_no_work: false,
            quiet: false,
            status_format: std::env::var("NINJA_STATUS").unwrap_or_else(|_| "[%f/%t] ".to_owned()),
            status_format_explicit: false,
            keep_depfile: false,
            keep_rsp: false,
            use_stat_cache: true,
            phony_cycle_error: false,
            max_load_average: 0.0,
            jobserver: None,
            use_jobserver: true,
        }
    }
}

fn guess_parallelism(processors: usize) -> usize {
    match processors {
        0 | 1 => 2,
        2 => 3,
        count => count.saturating_add(2),
    }
}

#[derive(Clone, Debug, Default)]
pub struct BuildOutcome {
    pub commands_run: usize,
    pub commands_failed: usize,
    pub edges_clean: usize,
    ran_edges: Vec<usize>,
}

#[derive(Debug)]
struct Completion {
    edge: usize,
    command: String,
    log_command: String,
    display: String,
    rspfile: Option<PathBuf>,
    output: io::Result<Output>,
    start_ms: u32,
    end_ms: u32,
    start_mtime: u128,
    prior_output_mtimes: Vec<Option<u128>>,
}

#[derive(Debug)]
struct LockFileGuard(PathBuf);

impl Drop for LockFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[derive(Clone, Copy, Debug)]
struct BuildLogEntry {
    command_hash: u64,
    mtime: u64,
    elapsed_ms: u32,
}

pub fn build_log_version(contents: &str) -> i32 {
    let Some(rest) = contents.strip_prefix("# ninja log v") else {
        return 0;
    };
    let rest = rest.trim_start_matches(|character: char| character.is_ascii_whitespace());
    let (negative, digits) = match rest.as_bytes().first() {
        Some(b'-') => (true, &rest[1..]),
        Some(b'+') => (false, &rest[1..]),
        _ => (false, rest),
    };
    let digits = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .map(char::from)
        .collect::<String>();
    if digits.is_empty() {
        return 0;
    }
    let magnitude = digits.parse::<u64>().unwrap_or(u64::MAX);
    if negative {
        -(magnitude.min(i32::MAX as u64 + 1) as i64) as i32
    } else {
        magnitude.min(i32::MAX as u64) as i32
    }
}

#[derive(Debug, Default)]
struct BuildLog<'a> {
    path: PathBuf,
    invalidation_warning: Option<&'static str>,
    entries: HashMap<&'a str, BuildLogEntry>,
}

#[derive(Clone, Debug, Default)]
struct StatCache<'a> {
    mtimes: HashMap<&'a str, Option<u128>>,
    dynamic: HashMap<String, Option<u128>>,
    may_have_missing_declared_sources: bool,
}

impl<'a> StatCache<'a> {
    fn preload(
        manifest: &'a Manifest,
        closure: &[usize],
        outputs: &HashMap<&str, usize>,
        discovered: &DiscoveredDeps,
        enabled: bool,
        check_missing_sources: bool,
    ) -> Result<Self, String> {
        if !enabled {
            return Ok(Self {
                may_have_missing_declared_sources: check_missing_sources,
                ..Self::default()
            });
        }
        let mut paths = HashSet::new();
        let mut declared_sources = HashSet::new();
        for edge_id in closure {
            let edge = &manifest.edges[*edge_id];
            paths.extend(edge.outputs());
            paths.extend(edge.inputs());
            if check_missing_sources {
                declared_sources
                    .extend(edge.inputs().filter(|input| !outputs.contains_key(*input)));
            }
        }
        let mut groups = HashMap::<&Path, Vec<&str>>::new();
        for path in &paths {
            let parent = Path::new(path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            groups.entry(parent).or_default().push(path);
        }

        let mut mtimes = HashMap::with_capacity(groups.values().map(Vec::len).sum::<usize>());
        for (directory, paths) in groups {
            if paths.len() < 8 {
                for path in paths {
                    mtimes.insert(path, checked_modified_ns(Path::new(path))?);
                }
                continue;
            }
            let entries = match directory_mtimes(directory) {
                DirectoryMtimes::Entries(entries) => entries,
                DirectoryMtimes::Missing => {
                    for path in paths {
                        mtimes.insert(path, None);
                    }
                    continue;
                }
                DirectoryMtimes::Unavailable => {
                    for path in paths {
                        mtimes.insert(path, checked_modified_ns(Path::new(path))?);
                    }
                    continue;
                }
            };
            for path in paths {
                let path_ref = Path::new(path);
                let entry_mtime = path_ref
                    .file_name()
                    .and_then(|name| entries.get(&directory_entry_key(name)).copied());
                #[cfg(windows)]
                let mtime = entry_mtime;
                #[cfg(not(windows))]
                let mtime = match entry_mtime {
                    Some(mtime) => Some(mtime),
                    None => checked_modified_ns(path_ref)?,
                };
                mtimes.insert(path, mtime);
            }
        }
        let mut dynamic = HashMap::new();
        for edge_id in closure {
            for path in &discovered.inputs[*edge_id] {
                if !mtimes.contains_key(path.as_str()) && !dynamic.contains_key(path) {
                    dynamic.insert(path.clone(), checked_modified_ns(Path::new(path))?);
                }
            }
        }
        let may_have_missing_declared_sources = declared_sources
            .into_iter()
            .any(|source| mtimes.get(source).is_none_or(Option::is_none));
        Ok(Self {
            mtimes,
            dynamic,
            may_have_missing_declared_sources,
        })
    }

    fn get(&mut self, path: &str) -> Option<u128> {
        if let Some(mtime) = self.mtimes.get(path) {
            return *mtime;
        }
        if let Some(mtime) = self.dynamic.get(path) {
            return *mtime;
        }
        let mtime = modified_ns(Path::new(path));
        self.dynamic.insert(path.to_owned(), mtime);
        mtime
    }

    fn checked_get(&mut self, path: &str) -> Result<Option<u128>, String> {
        if let Some(mtime) = self.mtimes.get(path) {
            return Ok(*mtime);
        }
        if let Some(mtime) = self.dynamic.get(path) {
            return Ok(*mtime);
        }
        let mtime = checked_modified_ns(Path::new(path))?;
        self.dynamic.insert(path.to_owned(), mtime);
        Ok(mtime)
    }

    fn mark_edge(&mut self, edge: &'a Edge, mtime: u128) {
        for output in edge.outputs() {
            self.mtimes.insert(output, Some(mtime));
        }
    }
}

enum DirectoryMtimes {
    Entries(HashMap<OsString, u128>),
    Missing,
    Unavailable,
}

#[cfg(windows)]
fn directory_entry_key(name: &std::ffi::OsStr) -> OsString {
    name.to_string_lossy().to_lowercase().into()
}

#[cfg(not(windows))]
fn directory_entry_key(name: &std::ffi::OsStr) -> OsString {
    name.to_owned()
}

#[cfg(windows)]
fn directory_mtimes(directory: &Path) -> DirectoryMtimes {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::{
        ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GetLastError,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch,
        FindFirstFileExW, FindNextFileW, WIN32_FIND_DATAW,
    };

    let pattern = directory.join("*");
    let mut wide = pattern.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    let mut data = WIN32_FIND_DATAW::default();
    // SAFETY: `wide` is null-terminated and `data` remains valid for the call.
    let mut handle = unsafe {
        FindFirstFileExW(
            wide.as_ptr(),
            FindExInfoBasic,
            (&raw mut data).cast(),
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // Large fetch is unsupported by some filesystems; retry without it.
        handle = unsafe {
            FindFirstFileExW(
                wide.as_ptr(),
                FindExInfoBasic,
                (&raw mut data).cast(),
                FindExSearchNameMatch,
                std::ptr::null(),
                0,
            )
        };
    }
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: GetLastError reads the calling thread's last-error slot.
        let error = unsafe { GetLastError() };
        return if matches!(
            error,
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_DIRECTORY
        ) {
            DirectoryMtimes::Missing
        } else {
            DirectoryMtimes::Unavailable
        };
    }

    let mut entries = HashMap::new();
    loop {
        let length = data
            .cFileName
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(data.cFileName.len());
        if length != 0 {
            let name = OsString::from_wide(&data.cFileName[..length]);
            let filetime = (u64::from(data.ftLastWriteTime.dwHighDateTime) << 32)
                | u64::from(data.ftLastWriteTime.dwLowDateTime);
            entries.insert(
                directory_entry_key(&name),
                filetime.saturating_sub(126_227_704_000_000_000) as u128,
            );
        }
        // SAFETY: `handle` is a valid enumeration handle and `data` is writable.
        if unsafe { FindNextFileW(handle, &raw mut data) } == 0 {
            break;
        }
    }
    // SAFETY: the enumeration handle is valid and closed exactly once.
    unsafe { FindClose(handle) };
    DirectoryMtimes::Entries(entries)
}

#[cfg(not(windows))]
fn directory_mtimes(directory: &Path) -> DirectoryMtimes {
    let mut entries = HashMap::new();
    let directory = match fs::read_dir(directory) {
        Ok(directory) => directory,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return DirectoryMtimes::Missing;
        }
        Err(_) => return DirectoryMtimes::Unavailable,
    };
    for entry in directory.flatten() {
        if let Ok(metadata) = entry.metadata()
            && let Some(mtime) = metadata_modified(&metadata)
        {
            entries.insert(directory_entry_key(&entry.file_name()), mtime);
        }
    }
    DirectoryMtimes::Entries(entries)
}

#[derive(Debug)]
struct DiscoveredDeps {
    inputs: Vec<Vec<String>>,
    missing: Vec<bool>,
    errors: Vec<Option<String>>,
    log: DepsLog,
}

impl DiscoveredDeps {
    #[cfg(test)]
    fn load(manifest: &Manifest) -> Self {
        Self::load_filtered(manifest, &vec![true; manifest.edges.len()]).unwrap()
    }

    fn load_for_build<'a>(
        manifest: &'a Manifest,
        outputs: &HashMap<&'a str, usize>,
        build_log: &BuildLog<'a>,
    ) -> Result<Self, String> {
        if !manifest.has_dependency_bindings() {
            return Self::load_filtered(manifest, &[]);
        }
        let declared_dirty = declared_dirty_edges(manifest, outputs, build_log);
        let load = declared_dirty
            .into_iter()
            .map(|dirty| !dirty)
            .collect::<Vec<_>>();
        Self::load_filtered(manifest, &load)
    }

    fn load_filtered(manifest: &Manifest, load: &[bool]) -> Result<Self, String> {
        let mut inputs = vec![Vec::new(); manifest.edges.len()];
        let mut missing = vec![false; manifest.edges.len()];
        let mut errors = vec![None; manifest.edges.len()];
        let builddir = manifest.variables.get("builddir").map(String::as_str);
        let path = deps_log_path(builddir);
        let log = DepsLog::load(path.clone())
            .map_err(|error| format!("loading deps log {}: {error}", path.display()))?;
        if log.was_invalidated() {
            eprintln!(
                "{}: warning: bad deps log signature or version; starting over",
                program_name()
            );
        }
        if !manifest.has_dependency_bindings() {
            return Ok(Self {
                inputs,
                missing,
                errors,
                log,
            });
        }
        for (edge_id, edge) in manifest.edges.iter().enumerate() {
            let deps = evaluate_binding(manifest, edge, "deps");
            let depfile = evaluate_unescaped_binding(manifest, edge, "depfile");
            if deps.is_empty() && depfile.is_empty() {
                continue;
            }
            if !deps.is_empty() {
                let entry = edge.outputs().next().and_then(|output| log.get(output));
                let valid = edge.outputs().next().is_some_and(|output| {
                    entry.is_some_and(|entry| {
                        modified_ns(Path::new(output))
                            .is_some_and(|mtime| mtime <= entry.mtime as u128)
                    })
                });
                if load[edge_id] && valid {
                    inputs[edge_id] = entry
                        .unwrap()
                        .inputs
                        .iter()
                        .cloned()
                        .map(canonicalize_owned_path)
                        .collect();
                } else if !valid {
                    missing[edge_id] = true;
                }
                continue;
            }
            if !depfile.is_empty() {
                if !load[edge_id] {
                    missing[edge_id] = !Path::new(&depfile).exists();
                    continue;
                }
                match fs::read_to_string(&depfile) {
                    Ok(contents) => match parse_depfile(&contents).map(normalize_depfile) {
                        Ok(parsed) if parsed.outputs.is_empty() => missing[edge_id] = true,
                        Ok(parsed)
                            if deps.is_empty()
                                && parsed.outputs.first().map(String::as_str)
                                    != edge.outputs().next() =>
                        {
                            missing[edge_id] = true;
                        }
                        Ok(parsed)
                            if deps.is_empty()
                                && parsed.outputs.iter().any(|output| {
                                    !edge.outputs().any(|declared| declared == output)
                                }) =>
                        {
                            let unexpected = parsed
                                .outputs
                                .iter()
                                .find(|output| !edge.outputs().any(|declared| declared == *output))
                                .unwrap();
                            errors[edge_id] = Some(format!(
                                "{depfile}: depfile mentions '{unexpected}' as an output, but no such output was declared"
                            ));
                        }
                        Ok(parsed) => {
                            inputs[edge_id] = parsed
                                .inputs
                                .into_iter()
                                .map(canonicalize_owned_path)
                                .collect();
                        }
                        Err(error) => errors[edge_id] = Some(format!("{depfile}: {error}")),
                    },
                    Err(_) => missing[edge_id] = true,
                }
            } else {
                missing[edge_id] = true;
            }
        }
        Ok(Self {
            inputs,
            missing,
            errors,
            log,
        })
    }

    fn record(&mut self, edge_id: usize, edge: &Edge, inputs: Vec<String>) -> Result<(), String> {
        for output in edge.outputs() {
            let mtime = checked_modified_ns(Path::new(output))
                .map_err(|error| format!("build stopped: {error}"))?
                .unwrap_or(0) as u64;
            self.log
                .record(output, mtime, &inputs)
                .map_err(|error| format!("writing dependency log: {error}"))?;
        }
        self.inputs[edge_id] = inputs;
        self.missing[edge_id] = false;
        Ok(())
    }
}

impl<'a> BuildLog<'a> {
    fn load(path: PathBuf, outputs: &HashMap<&'a str, usize>) -> io::Result<Self> {
        let mut log = Self {
            path,
            invalidation_warning: None,
            entries: HashMap::new(),
        };
        let mut contents = match fs::read_to_string(&log.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(log),
            #[cfg(unix)]
            Err(error) if error.kind() == io::ErrorKind::IsADirectory => String::new(),
            Err(error) => return Err(error),
        };
        let version = build_log_version(&contents);
        if !contents.is_empty() && version != 7 {
            log.invalidation_warning = Some(if version > 7 {
                "build log version is too new; starting over"
            } else {
                "build log version is too old; starting over"
            });
            let _ = fs::remove_file(&log.path);
            return Ok(log);
        }
        if !contents.is_empty()
            && !contents.ends_with('\n')
            && let Some(last_newline) = contents.rfind('\n')
        {
            contents.truncate(last_newline + 1);
            let _ = OpenOptions::new()
                .write(true)
                .open(&log.path)
                .and_then(|file| file.set_len(contents.len() as u64));
        }
        let mut total_entries = 0usize;
        let mut dead_outputs = HashSet::new();
        for line in contents.lines().skip(1) {
            let mut fields = line.split('\t');
            let Some(start_ms) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
                continue;
            };
            let Some(end_ms) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
                continue;
            };
            let Some(mtime) = fields.next().and_then(|value| value.parse::<u64>().ok()) else {
                continue;
            };
            let Some(output) = fields.next() else {
                continue;
            };
            let Some(command_hash) = fields.next() else {
                continue;
            };
            let Ok(command_hash) = u64::from_str_radix(command_hash, 16) else {
                continue;
            };
            total_entries += 1;
            if let Some((&output, _)) = outputs.get_key_value(output) {
                log.entries.insert(
                    output,
                    BuildLogEntry {
                        command_hash,
                        mtime,
                        elapsed_ms: end_ms.saturating_sub(start_ms),
                    },
                );
            } else {
                dead_outputs.insert(output);
            }
        }
        let unique_outputs = log.entries.len() + dead_outputs.len();
        if total_entries > 100 && total_entries > unique_outputs * 3 {
            let _ = recompact_build_log(&log.path, &contents, outputs);
        }
        Ok(log)
    }

    fn command_changed(&self, edge: &Edge, command: &str, generator: bool) -> bool {
        if generator {
            return false;
        }
        let command_hash = hash(command.as_bytes());
        edge.outputs().any(|output| {
            self.entries
                .get(output)
                .is_none_or(|entry| entry.command_hash != command_hash)
        })
    }

    fn recorded_mtime_dirty<'edge>(
        &self,
        edge: &'edge Edge,
        newest_input: u128,
    ) -> Option<&'edge str> {
        edge.outputs().find(|output| {
            self.entries
                .get(*output)
                .is_some_and(|entry| u128::from(entry.mtime) < newest_input)
        })
    }

    fn has_entry(&self, output: &str) -> bool {
        self.entries.contains_key(output)
    }

    fn previous_elapsed(&self, edge: &Edge) -> Option<u32> {
        edge.outputs()
            .find_map(|output| self.entries.get(output).map(|entry| entry.elapsed_ms))
    }

    fn recorded_mtime(&self, output: &str) -> Option<u64> {
        self.entries.get(output).map(|entry| entry.mtime)
    }

    fn record(
        &mut self,
        edge: &'a Edge,
        command: &str,
        start_ms: u32,
        end_ms: u32,
        record_mtime: u128,
    ) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let new_file = !self.path.exists() || self.path.metadata()?.len() == 0;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        if new_file {
            writeln!(file, "# ninja log v7")?;
        }
        let command_hash = hash(command.as_bytes());
        let record_mtime = record_mtime.min(u128::from(u64::MAX)) as u64;
        for output in edge.outputs() {
            writeln!(
                file,
                "{}\t{}\t{}\t{}\t{:x}",
                start_ms, end_ms, record_mtime, output, command_hash
            )?;
            self.entries.insert(
                output,
                BuildLogEntry {
                    command_hash,
                    mtime: record_mtime,
                    elapsed_ms: end_ms.saturating_sub(start_ms),
                },
            );
        }
        Ok(())
    }
}

fn declared_dirty_edges<'a>(
    manifest: &'a Manifest,
    outputs: &HashMap<&'a str, usize>,
    build_log: &BuildLog<'a>,
) -> Vec<bool> {
    fn path_mtime<'a>(
        path: &str,
        manifest: &'a Manifest,
        outputs: &HashMap<&'a str, usize>,
        visiting: &mut HashSet<usize>,
    ) -> Option<u128> {
        if let Some(mtime) = modified_ns(Path::new(path)) {
            return Some(mtime);
        }
        let edge_id = outputs.get(path).copied()?;
        let edge = &manifest.edges[edge_id];
        if edge.rule != "phony" || !visiting.insert(edge_id) {
            return None;
        }
        if edge.inputs().next().is_none() && edge.validations.is_empty() {
            visiting.remove(&edge_id);
            return Some(u128::MAX);
        }
        let mut newest = 0;
        for input in edge.explicit_inputs.iter().chain(&edge.implicit_inputs) {
            newest = newest.max(path_mtime(input, manifest, outputs, visiting)?);
        }
        visiting.remove(&edge_id);
        Some(newest)
    }

    fn visit<'a>(
        edge_id: usize,
        manifest: &'a Manifest,
        outputs: &HashMap<&'a str, usize>,
        build_log: &BuildLog<'a>,
        state: &mut [u8],
        result: &mut [bool],
    ) -> bool {
        match state[edge_id] {
            1 => return true,
            2 => return result[edge_id],
            _ => state[edge_id] = 1,
        }
        let edge = &manifest.edges[edge_id];
        let mut dirty = edge.rule == "phony" && edge.inputs().next().is_none();
        let mut oldest_output = u128::MAX;
        let mut newest_input = 0;
        if edge.rule != "phony" {
            for output in edge.outputs() {
                let Some(mtime) = modified_ns(Path::new(output)) else {
                    dirty = true;
                    continue;
                };
                oldest_output = oldest_output.min(mtime);
            }
        }

        for input in edge.explicit_inputs.iter().chain(&edge.implicit_inputs) {
            if let Some(producer) = outputs.get(input.as_str()).copied()
                && visit(producer, manifest, outputs, build_log, state, result)
            {
                dirty = true;
            }
            let Some(mtime) = path_mtime(input, manifest, outputs, &mut HashSet::new()) else {
                dirty = true;
                continue;
            };
            newest_input = newest_input.max(mtime);
        }

        if edge.rule != "phony" {
            let restat = truthy(&evaluate_binding(manifest, edge, "restat"));
            let use_restat = restat && edge.outputs().all(|output| build_log.has_entry(output));
            if !use_restat && newest_input > oldest_output {
                dirty = true;
            }
            let rspfile_content = evaluate_binding(manifest, edge, "rspfile_content");
            let command = evaluate_binding(manifest, edge, "command");
            let log_command = if rspfile_content.is_empty() {
                command
            } else {
                format!("{command};rspfile={rspfile_content}")
            };
            let generator = truthy(&evaluate_binding(manifest, edge, "generator"));
            if build_log.command_changed(edge, &log_command, generator)
                || build_log.recorded_mtime_dirty(edge, newest_input).is_some()
            {
                dirty = true;
            }
        }
        state[edge_id] = 2;
        result[edge_id] = dirty;
        dirty
    }

    let mut state = vec![0; manifest.edges.len()];
    let mut result = vec![false; manifest.edges.len()];
    for (edge_id, edge) in manifest.edges.iter().enumerate() {
        if !evaluate_binding(manifest, edge, "deps").is_empty()
            || !evaluate_unescaped_binding(manifest, edge, "depfile").is_empty()
        {
            visit(
                edge_id,
                manifest,
                outputs,
                build_log,
                &mut state,
                &mut result,
            );
        }
    }
    result
}

fn recompact_build_log(
    path: &Path,
    contents: &str,
    live_outputs: &HashMap<&str, usize>,
) -> io::Result<()> {
    let mut latest = HashMap::<&str, &str>::new();
    for line in contents.lines().skip(1) {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() == 5
            && fields[0].parse::<u32>().is_ok()
            && fields[1].parse::<u32>().is_ok()
            && fields[2].parse::<u64>().is_ok()
            && u64::from_str_radix(fields[4], 16).is_ok()
        {
            latest.insert(fields[3], line);
        }
    }
    let mut outputs = latest
        .keys()
        .copied()
        .filter(|output| live_outputs.contains_key(*output) || Path::new(output).exists())
        .collect::<Vec<_>>();
    outputs.sort_unstable();
    let mut compacted = String::from("# ninja log v7\n");
    for output in outputs {
        compacted.push_str(latest[output]);
        compacted.push('\n');
    }
    fs::write(path, compacted)
}

fn build_log_path(manifest: &Manifest) -> PathBuf {
    manifest.variables.get("builddir").map_or_else(
        || PathBuf::from(".ninja_log"),
        |builddir| {
            if builddir.is_empty() {
                PathBuf::from(".ninja_log")
            } else {
                Path::new(builddir).join(".ninja_log")
            }
        },
    )
}

pub fn run_build(
    manifest: &Manifest,
    requested_targets: &[String],
    options: &BuildOptions,
) -> Result<BuildOutcome, String> {
    LAST_BUILD_EXIT_CODE.with(|code| code.set(0));
    ensure_build_directory(manifest, options.dry_run)?;
    let phase = Instant::now();
    let initial_output_map = output_map(manifest);
    let output_map_time = phase.elapsed();
    let phase = Instant::now();
    let initial_build_log_path = build_log_path(manifest);
    let build_log =
        BuildLog::load(initial_build_log_path.clone(), &initial_output_map).map_err(|error| {
            format!(
                "loading build log {}: {error}",
                initial_build_log_path.display()
            )
        })?;
    if let Some(warning) = build_log.invalidation_warning {
        eprintln!("{}: warning: {warning}", program_name());
    }
    let build_log_time = phase.elapsed();
    let phase = Instant::now();
    let discovered = DiscoveredDeps::load_for_build(manifest, &initial_output_map, &build_log)?;
    let deps_time = phase.elapsed();
    let phase = Instant::now();
    let targets = select_targets(
        manifest,
        requested_targets,
        &initial_output_map,
        &discovered.log,
    )?;
    let closure = dependency_closure(
        manifest,
        &targets,
        &initial_output_map,
        &discovered,
        options.phony_cycle_error,
    )?;
    let closure_time = phase.elapsed();
    let mut dyndep_files = Vec::new();
    let mut unique_dyndep_files = HashSet::new();
    for &edge_id in &closure {
        let edge = &manifest.edges[edge_id];
        let dyndep = evaluate_unescaped_binding(manifest, edge, "dyndep");
        if dyndep.is_empty() {
            continue;
        }
        if !edge.inputs().any(|input| input == dyndep) {
            return Err(format!(
                "dyndep file '{dyndep}' is not an input of '{}'",
                edge_label(edge)
            ));
        }
        if unique_dyndep_files.insert(dyndep.clone()) {
            dyndep_files.push(dyndep);
        }
    }
    if dyndep_files.is_empty() {
        return run_build_prepared(
            manifest,
            options,
            PreparedBuild {
                output_map: initial_output_map,
                discovered,
                build_log,
                closure,
                stats: PreparationStats {
                    output_map: output_map_time,
                    dependencies: deps_time,
                    closure: closure_time,
                    build_log: build_log_time,
                },
            },
            ProgressContext::default(),
        );
    }

    let mut prebuild_options = options.clone();
    prebuild_options.quiet_no_work = true;
    prebuild_options.quiet = true;
    let mut expanded = manifest.clone();
    let mut seen_dyndeps = HashSet::<String>::new();
    let mut loaded_dyndeps = Vec::new();
    let mut prebuild = BuildOutcome::default();
    let mut dyndep_graph_time = Duration::ZERO;
    let mut dyndep_prebuild_time = Duration::ZERO;
    let mut dyndep_load_time = Duration::ZERO;
    loop {
        let dyndep_phase = Instant::now();
        let phase = Instant::now();
        let current_output_map = output_map(&expanded);
        let current_output_map_time = phase.elapsed();
        let phase = Instant::now();
        let current_build_log_path = build_log_path(&expanded);
        let current_build_log = BuildLog::load(current_build_log_path.clone(), &current_output_map)
            .map_err(|error| {
                format!(
                    "loading build log {}: {error}",
                    current_build_log_path.display()
                )
            })?;
        if let Some(warning) = current_build_log.invalidation_warning {
            eprintln!("{}: warning: {warning}", program_name());
        }
        let current_build_log_time = phase.elapsed();
        let phase = Instant::now();
        let current_discovered =
            DiscoveredDeps::load_for_build(&expanded, &current_output_map, &current_build_log)?;
        let current_dependencies_time = phase.elapsed();
        let phase = Instant::now();
        let current_targets = select_targets(
            &expanded,
            requested_targets,
            &current_output_map,
            &current_discovered.log,
        )?;
        let current_closure = dependency_closure(
            &expanded,
            &current_targets,
            &current_output_map,
            &current_discovered,
            options.phony_cycle_error,
        )?;
        let current_closure_time = phase.elapsed();
        dyndep_files = pending_dyndep_files(&expanded, &current_closure, &seen_dyndeps)?;
        if dyndep_files.is_empty() {
            break;
        }
        let prebuild_targets = dyndep_prebuild_targets(
            &expanded,
            &current_closure,
            &current_output_map,
            &current_discovered,
            &dyndep_files,
        );

        dyndep_graph_time += dyndep_phase.elapsed();
        let pass = if prebuild_targets.is_empty() {
            BuildOutcome::default()
        } else {
            let prebuild_target_edges = select_targets(
                &expanded,
                &prebuild_targets,
                &current_output_map,
                &current_discovered.log,
            )?;
            let prebuild_closure = dependency_closure(
                &expanded,
                &prebuild_target_edges,
                &current_output_map,
                &current_discovered,
                options.phony_cycle_error,
            )?;
            let dyndep_phase = Instant::now();
            let pass = run_build_prepared(
                &expanded,
                &prebuild_options,
                PreparedBuild {
                    output_map: current_output_map,
                    discovered: current_discovered,
                    build_log: current_build_log,
                    closure: prebuild_closure,
                    stats: PreparationStats {
                        output_map: current_output_map_time,
                        dependencies: current_dependencies_time,
                        closure: current_closure_time,
                        build_log: current_build_log_time,
                    },
                },
                ProgressContext::default(),
            )?;
            dyndep_prebuild_time += dyndep_phase.elapsed();
            pass
        };
        let known_dyndeps = expanded
            .edges
            .iter()
            .filter_map(|edge| {
                let path = evaluate_unescaped_binding(&expanded, edge, "dyndep");
                (!path.is_empty()).then_some(path)
            })
            .collect::<HashSet<_>>();
        let mut apply_files = pass
            .ran_edges
            .iter()
            .flat_map(|edge| expanded.edges[*edge].outputs())
            .filter(|output| known_dyndeps.contains(*output))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        apply_files.extend(dyndep_files);
        apply_files.retain(|file| seen_dyndeps.insert(file.clone()));
        loaded_dyndeps.extend(apply_files.iter().cloned());

        prebuild.commands_run += pass.commands_run;
        prebuild.commands_failed += pass.commands_failed;
        prebuild.edges_clean += pass.edges_clean;
        prebuild.ran_edges.extend(pass.ran_edges);
        let dyndep_phase = Instant::now();
        if let Err(error) = apply_dyndep_files(&mut expanded, &apply_files) {
            let total = current_closure
                .iter()
                .filter(|edge| expanded.edges[**edge].rule != "phony")
                .count();
            print_prior_statuses(&expanded, options, &prebuild.ran_edges, total)?;
            return Err(error);
        }
        dyndep_load_time += dyndep_phase.elapsed();
    }
    if options.stats {
        eprintln!(
            "{} stats: dyndep graph             {:>9.3} ms",
            program_name(),
            dyndep_graph_time.as_secs_f64() * 1000.0
        );
        eprintln!(
            "{} stats: dyndep prebuild          {:>9.3} ms",
            program_name(),
            dyndep_prebuild_time.as_secs_f64() * 1000.0
        );
        eprintln!(
            "{} stats: dyndep load/apply        {:>9.3} ms",
            program_name(),
            dyndep_load_time.as_secs_f64() * 1000.0
        );
    }
    let mut final_options = options.clone();
    final_options.quiet_no_work |= prebuild.commands_run > 0;
    let offset = prebuild.commands_run + prebuild.commands_failed;
    let mut outcome = run_build_internal(
        &expanded,
        requested_targets,
        &final_options,
        ProgressContext {
            offset,
            completed_edges: prebuild.ran_edges.clone(),
            prefix_edges: prebuild.ran_edges.clone(),
            loaded_dyndeps,
        },
    )?;
    outcome.commands_run += prebuild.commands_run;
    outcome.commands_failed += prebuild.commands_failed;
    outcome.edges_clean += prebuild.edges_clean;
    Ok(outcome)
}

fn pending_dyndep_files(
    manifest: &Manifest,
    closure: &[usize],
    seen: &HashSet<String>,
) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let mut pending = HashSet::new();
    for &edge_id in closure {
        let edge = &manifest.edges[edge_id];
        let dyndep = evaluate_unescaped_binding(manifest, edge, "dyndep");
        if dyndep.is_empty() {
            continue;
        }
        if !edge.inputs().any(|input| input == dyndep) {
            return Err(format!(
                "dyndep file '{dyndep}' is not an input of '{}'",
                edge_label(edge)
            ));
        }
        if !seen.contains(&dyndep) && pending.insert(dyndep.clone()) {
            files.push(dyndep);
        }
    }
    Ok(files)
}

fn dyndep_prebuild_targets(
    manifest: &Manifest,
    closure: &[usize],
    outputs: &HashMap<&str, usize>,
    discovered: &DiscoveredDeps,
    dyndep_files: &[String],
) -> Vec<String> {
    let pending_files = dyndep_files
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut in_closure = vec![false; manifest.edges.len()];
    for edge in closure {
        in_closure[*edge] = true;
    }
    let mut dependents = vec![Vec::new(); manifest.edges.len()];
    for &consumer in closure {
        let edge = &manifest.edges[consumer];
        for input in edge
            .inputs()
            .chain(discovered.inputs[consumer].iter().map(String::as_str))
        {
            if let Some(producer) = outputs.get(input).copied()
                && in_closure[producer]
            {
                dependents[producer].push(consumer);
            }
        }
        for validation in &edge.validations {
            if let Some(validation_edge) = outputs.get(validation.as_str()).copied()
                && in_closure[validation_edge]
            {
                // Selecting the consumer would also select its validation.
                dependents[validation_edge].push(consumer);
            }
        }
    }

    let mut unsafe_edge = vec![false; manifest.edges.len()];
    let mut queue = VecDeque::new();
    for &edge_id in closure {
        let dyndep = evaluate_unescaped_binding(manifest, &manifest.edges[edge_id], "dyndep");
        if pending_files.contains(dyndep.as_str()) {
            unsafe_edge[edge_id] = true;
            queue.push_back(edge_id);
        }
    }
    while let Some(edge_id) = queue.pop_front() {
        for &dependent in &dependents[edge_id] {
            if !unsafe_edge[dependent] {
                unsafe_edge[dependent] = true;
                queue.push_back(dependent);
            }
        }
    }

    let mut targets = dyndep_files
        .iter()
        .filter(|file| outputs.contains_key(file.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return targets;
    }
    let mut selected = targets.iter().cloned().collect::<HashSet<_>>();
    for &edge_id in closure {
        let edge = &manifest.edges[edge_id];
        if !unsafe_edge[edge_id]
            && edge.rule != "phony"
            && !edge
                .inputs()
                .any(|input| !outputs.contains_key(input) && !Path::new(input).exists())
            && let Some(output) = edge.outputs().next()
            && selected.insert(output.to_owned())
        {
            targets.push(output.to_owned());
        }
    }
    targets
}

fn print_prior_statuses(
    manifest: &Manifest,
    options: &BuildOptions,
    edges: &[usize],
    total: usize,
) -> Result<(), String> {
    if options.quiet {
        return Ok(());
    }
    let mut status = StatusFormatter::new(options.jobs);
    let mut printer = BuildOutput::new(!options.verbose && !options.quiet, options.dry_run);
    for (index, edge_id) in edges.iter().copied().enumerate() {
        let edge = &manifest.edges[edge_id];
        let command = evaluate_binding(manifest, edge, "command");
        let description = evaluate_binding(manifest, edge, "description");
        let display = if options.verbose || description.is_empty() {
            &command
        } else {
            &description
        };
        let line = status.format(
            &options.status_format,
            options.status_format_explicit,
            StatusSnapshot {
                started: index + 1,
                finished: index + 1,
                running: 1,
                total,
                description: display,
                elapsed: Duration::ZERO,
            },
        )?;
        printer.print_status(&line, options.verbose)?;
    }
    printer.finish_line()
}

pub fn apply_dyndep_files(manifest: &mut Manifest, files: &[String]) -> Result<(), String> {
    let mut outputs = output_map(manifest)
        .into_iter()
        .map(|(path, edge)| (path.to_owned(), edge))
        .collect::<HashMap<_, _>>();
    let edge_dyndeps = manifest
        .edges
        .iter()
        .map(|edge| canonicalize_owned_path(evaluate_unescaped_binding(manifest, edge, "dyndep")))
        .collect::<Vec<_>>();
    let mut bound_edges = HashMap::<&str, Vec<usize>>::new();
    for (edge_id, dyndep) in edge_dyndeps.iter().enumerate() {
        if !dyndep.is_empty() {
            bound_edges.entry(dyndep).or_default().push(edge_id);
        }
    }
    let parsed_files = load_dyndep_records(files);
    for (file, records) in files.iter().zip(parsed_files) {
        let canonical_file = canonicalize_owned_path(file.clone());
        let records = records?;
        let mut seen_edges = HashSet::new();
        let mut normalized_records = Vec::with_capacity(records.len());
        for mut record in records {
            record.output = canonicalize_owned_path(record.output);
            for path in record
                .implicit_inputs
                .iter_mut()
                .chain(&mut record.implicit_outputs)
            {
                *path = canonicalize_owned_path(std::mem::take(path));
            }
            let edge_id = outputs.get(&record.output).copied().ok_or_else(|| {
                format!("{file}: no build statement exists for '{}'", record.output)
            })?;
            if !seen_edges.insert(edge_id) {
                return Err(format!(
                    "{file}: multiple statements for '{}'",
                    record.output
                ));
            }
            normalized_records.push((edge_id, record));
        }

        for &edge_id in bound_edges
            .get(canonical_file.as_str())
            .into_iter()
            .flatten()
        {
            if !seen_edges.contains(&edge_id) {
                let edge = &manifest.edges[edge_id];
                return Err(format!(
                    "'{}' not mentioned in its dyndep file '{file}'",
                    edge.outputs().next().unwrap_or_default()
                ));
            }
        }

        for (edge_id, record) in normalized_records {
            if edge_dyndeps[edge_id] != canonical_file {
                return Err(format!(
                    "dyndep file '{file}' mentions output '{}' whose build statement does not have a dyndep binding for the file",
                    manifest.edges[edge_id]
                        .outputs()
                        .next()
                        .unwrap_or(&record.output)
                ));
            }
            let edge = &mut manifest.edges[edge_id];
            for input in record.implicit_inputs {
                if !edge.implicit_inputs.contains(&input) {
                    edge.implicit_inputs.push(input);
                }
            }
            for output in record.implicit_outputs {
                if outputs.contains_key(&output) {
                    return Err(format!("multiple rules generate {output}"));
                }
                if !edge.implicit_outputs.contains(&output) {
                    edge.implicit_outputs.push(output.clone());
                }
                outputs.insert(output, edge_id);
            }
            if record.restat {
                edge.bindings.insert("restat".to_owned(), "1".to_owned());
            }
        }
    }
    Ok(())
}

fn load_dyndep_records(files: &[String]) -> Vec<Result<Vec<DyndepRecord>, String>> {
    if files.len() < 16 {
        return files.iter().map(|file| load_dyndep_record(file)).collect();
    }
    let middle = files.len().div_ceil(2);
    thread::scope(|scope| {
        let first = scope.spawn(|| {
            files[..middle]
                .iter()
                .map(|file| load_dyndep_record(file))
                .collect::<Vec<_>>()
        });
        let second = scope.spawn(|| {
            files[middle..]
                .iter()
                .map(|file| load_dyndep_record(file))
                .collect::<Vec<_>>()
        });
        let mut records = first.join().expect("dyndep loader worker panicked");
        records.extend(second.join().expect("dyndep loader worker panicked"));
        records
    })
}

fn load_dyndep_record(file: &str) -> Result<Vec<DyndepRecord>, String> {
    let source = load_dyndep_source(file)?;
    parse_dyndep(&source, Path::new(file))
}

#[cfg(windows)]
fn load_dyndep_source(file: &str) -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileA, CreateFileW, FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_READ, OPEN_EXISTING,
        ReadFile,
    };

    let mut narrow = [0u8; 512];
    let handle = if file.is_ascii() && file.len() < narrow.len() {
        narrow[..file.len()].copy_from_slice(file.as_bytes());
        // SAFETY: `narrow` is zero-initialized after the copied ASCII path and
        // the remaining arguments follow the documented read-only contract.
        unsafe {
            CreateFileA(
                narrow.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_SEQUENTIAL_SCAN,
                std::ptr::null_mut(),
            )
        }
    } else {
        let wide = Path::new(file)
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: `wide` is NUL-terminated and the remaining arguments follow
        // the documented read-only CreateFile contract.
        unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_SEQUENTIAL_SCAN,
                std::ptr::null_mut(),
            )
        }
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(dyndep_load_error(file, &io::Error::last_os_error()));
    }

    let mut source = Vec::new();
    let mut buffer = [std::mem::MaybeUninit::<u8>::uninit(); 64 << 10];
    loop {
        let mut read = 0;
        // SAFETY: `handle` is open, and `buffer` provides the writable byte
        // range described by its length for this synchronous read.
        if unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr().cast::<u8>(),
                buffer.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            // SAFETY: `handle` remains valid until it is closed exactly once.
            unsafe { CloseHandle(handle) };
            return Err(dyndep_load_error(file, &error));
        }
        if read == 0 {
            break;
        }
        // SAFETY: ReadFile initialized exactly the reported prefix.
        let initialized =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), read as usize) };
        source.extend_from_slice(initialized);
    }
    // SAFETY: `handle` remains valid until it is closed exactly once.
    unsafe { CloseHandle(handle) };
    String::from_utf8(source).map_err(|_| {
        dyndep_load_error(
            file,
            &io::Error::new(
                io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            ),
        )
    })
}

#[cfg(not(windows))]
fn load_dyndep_source(file: &str) -> Result<String, String> {
    fs::read_to_string(file).map_err(|error| dyndep_load_error(file, &error))
}

fn dyndep_load_error(file: &str, error: &io::Error) -> String {
    let message = error.to_string();
    let message = message
        .rfind(" (os error ")
        .map_or(message.as_str(), |suffix| &message[..suffix]);
    #[cfg(windows)]
    return format!("loading '{file}': {message}\r\r\n");
    #[cfg(not(windows))]
    format!("loading '{file}': {message}")
}

pub fn manifest_with_existing_dyndeps(manifest: &Manifest) -> Result<Option<Manifest>, String> {
    let mut seen = HashSet::new();
    let files = manifest
        .edges
        .iter()
        .filter_map(|edge| {
            let path = evaluate_unescaped_binding(manifest, edge, "dyndep");
            (!path.is_empty() && Path::new(&path).exists() && seen.insert(path.clone()))
                .then_some(path)
        })
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Ok(None);
    }
    let mut expanded = manifest.clone();
    apply_dyndep_files(&mut expanded, &files)?;
    Ok(Some(expanded))
}

fn run_build_internal(
    manifest: &Manifest,
    requested_targets: &[String],
    options: &BuildOptions,
    progress: ProgressContext,
) -> Result<BuildOutcome, String> {
    let phase = Instant::now();
    let output_map = output_map(manifest);
    let output_map_time = phase.elapsed();
    let phase = Instant::now();
    let build_log_file = build_log_path(manifest);
    let build_log = BuildLog::load(build_log_file.clone(), &output_map)
        .map_err(|error| format!("loading build log {}: {error}", build_log_file.display()))?;
    if let Some(warning) = build_log.invalidation_warning {
        eprintln!("{}: warning: {warning}", program_name());
    }
    let build_log_time = phase.elapsed();
    let phase = Instant::now();
    let discovered = DiscoveredDeps::load_for_build(manifest, &output_map, &build_log)?;
    let deps_time = phase.elapsed();
    let phase = Instant::now();
    let targets = select_targets(manifest, requested_targets, &output_map, &discovered.log)?;
    let closure = dependency_closure(
        manifest,
        &targets,
        &output_map,
        &discovered,
        options.phony_cycle_error,
    )?;
    let closure_time = phase.elapsed();
    run_build_prepared(
        manifest,
        options,
        PreparedBuild {
            output_map,
            discovered,
            build_log,
            closure,
            stats: PreparationStats {
                output_map: output_map_time,
                dependencies: deps_time,
                closure: closure_time,
                build_log: build_log_time,
            },
        },
        progress,
    )
}

#[derive(Clone, Copy, Debug, Default)]
struct PreparationStats {
    output_map: Duration,
    dependencies: Duration,
    closure: Duration,
    build_log: Duration,
}

struct PreparedBuild<'a> {
    output_map: HashMap<&'a str, usize>,
    discovered: DiscoveredDeps,
    build_log: BuildLog<'a>,
    closure: Vec<usize>,
    stats: PreparationStats,
}

#[derive(Clone, Debug, Default)]
struct ProgressContext {
    offset: usize,
    completed_edges: Vec<usize>,
    prefix_edges: Vec<usize>,
    loaded_dyndeps: Vec<String>,
}

fn run_build_prepared<'a>(
    manifest: &'a Manifest,
    options: &BuildOptions,
    prepared: PreparedBuild<'a>,
    progress: ProgressContext,
) -> Result<BuildOutcome, String> {
    let PreparedBuild {
        output_map,
        mut discovered,
        mut build_log,
        closure,
        stats: preparation,
    } = prepared;
    let scheduler_setup_start = Instant::now();
    let mut in_closure = vec![false; manifest.edges.len()];
    for edge in &closure {
        in_closure[*edge] = true;
    }

    let mut pending = vec![0usize; manifest.edges.len()];
    let mut dependents = Dependents::new(
        manifest.edges.len(),
        closure.len(),
        manifest.has_pool_bindings(),
    );
    if manifest.has_pool_bindings() {
        let mut dependency_links = vec![Vec::<(usize, usize)>::new(); manifest.edges.len()];
        for edge_id in 0..manifest.edges.len() {
            if !in_closure[edge_id] {
                continue;
            }
            let mut producers = HashSet::new();
            for input in manifest.edges[edge_id]
                .inputs()
                .chain(discovered.inputs[edge_id].iter().map(String::as_str))
            {
                let Some(&producer) = output_map.get(input) else {
                    continue;
                };
                if !in_closure[producer]
                    || (producer == edge_id
                        && tolerates_phony_self_reference(
                            &manifest.edges[edge_id],
                            options.phony_cycle_error,
                        ))
                {
                    continue;
                }
                let output_position = manifest.edges[producer]
                    .outputs()
                    .position(|output| output == input)
                    .unwrap_or(0);
                if producers.insert(producer) {
                    pending[edge_id] += 1;
                    dependency_links[producer].push((output_position, edge_id));
                } else if let Some((position, _)) = dependency_links[producer].last_mut() {
                    *position = (*position).min(output_position);
                }
            }
        }
        for (producer, links) in dependency_links.iter_mut().enumerate() {
            links.sort_unstable();
            for &(_, dependent) in links.iter() {
                dependents.add(producer, dependent);
            }
        }
    } else {
        for &edge_id in &closure {
            let mut seen = HashSet::new();
            for input in manifest.edges[edge_id]
                .inputs()
                .chain(discovered.inputs[edge_id].iter().map(String::as_str))
            {
                if let Some(&producer) = output_map.get(input)
                    && in_closure[producer]
                    && seen.insert(producer)
                {
                    if producer == edge_id
                        && tolerates_phony_self_reference(
                            &manifest.edges[edge_id],
                            options.phony_cycle_error,
                        )
                    {
                        continue;
                    }
                    pending[edge_id] += 1;
                    dependents.add(producer, edge_id);
                }
            }
        }
    }
    let scheduler_setup_time = scheduler_setup_start.elapsed();

    let builddir = manifest
        .variables
        .get("builddir")
        .map_or("", String::as_str);
    let lock_path = if builddir.is_empty() {
        PathBuf::from(".ninja_lock")
    } else {
        Path::new(builddir).join(".ninja_lock")
    };
    let _lock_file = LockFileGuard(lock_path.clone());
    let phase = Instant::now();
    let mut stat_cache = StatCache::preload(
        manifest,
        &closure,
        &output_map,
        &discovered,
        options.use_stat_cache,
        true,
    )?;
    let stat_time = phase.elapsed();
    if stat_cache.may_have_missing_declared_sources {
        let mut deferred_phony_order_only = Vec::new();
        for &edge_id in &closure {
            let edge = &manifest.edges[edge_id];
            for input in edge.explicit_inputs.iter().chain(&edge.implicit_inputs) {
                if !output_map.contains_key(input.as_str())
                    && stat_cache.checked_get(input)?.is_none()
                {
                    return Err(format!(
                        "'{input}', needed by '{}', missing and no known rule to make it",
                        edge.outputs().next().unwrap_or(input)
                    ));
                }
            }
            for input in &edge.order_only_inputs {
                if !output_map.contains_key(input.as_str())
                    && stat_cache.checked_get(input)?.is_none()
                {
                    if edge.rule == "phony" {
                        deferred_phony_order_only.push((edge_id, input));
                    } else {
                        return Err(format!(
                            "'{input}', needed by '{}', missing and no known rule to make it",
                            edge.outputs().next().unwrap_or(input)
                        ));
                    }
                }
            }
        }
        if !deferred_phony_order_only.is_empty() {
            let dirty = initially_dirty_edges(
                manifest,
                &closure,
                &output_map,
                &build_log,
                &discovered,
                &stat_cache,
                true,
            );
            for (edge_id, input) in deferred_phony_order_only {
                let edge = &manifest.edges[edge_id];
                let waits_for_dirty_producer = edge.inputs().any(|dependency| {
                    output_map
                        .get(dependency)
                        .is_some_and(|producer| dirty[*producer])
                });
                if dirty[edge_id] || waits_for_dirty_producer {
                    return Err(format!(
                        "'{input}', needed by '{}', missing and no known rule to make it",
                        edge.outputs().next().unwrap_or(input)
                    ));
                }
            }
        }
    }
    let critical_path = critical_path_weights(manifest, &closure, &output_map, &discovered);
    let mut ready = closure
        .iter()
        .copied()
        .filter(|edge| pending[*edge] == 0)
        .map(|edge| (critical_path[edge], Reverse(edge)))
        .collect::<BinaryHeap<_>>();
    let mut newly_ready = Vec::new();
    let mut finished = vec![false; manifest.edges.len()];
    let mut failed_prerequisite = vec![false; manifest.edges.len()];
    let mut ran = vec![false; manifest.edges.len()];
    let mut restat_cleaned_outputs = HashSet::new();
    let mut running = 0usize;
    let mut implicit_job_slot = options.jobserver.is_some();
    let mut job_slots = HashMap::<usize, JobSlot>::new();
    let mut pool_usage = HashMap::<String, usize>::new();
    let mut pool_waiting = HashMap::<String, BinaryHeap<(usize, Reverse<usize>)>>::new();
    let mut pool_reserved = vec![false; manifest.edges.len()];
    let mut outcome = BuildOutcome::default();
    let mut failures = Vec::new();
    let mut stop_starting = false;
    let mut commands_started = progress.offset;
    let mut dry_run_statuses = Vec::new();
    let mut dry_run_pending = VecDeque::<usize>::new();
    let mut command_buffer = String::new();
    let start = Instant::now();
    let (tx, rx) = mpsc::channel::<Completion>();
    let mut initially_dirty = None;
    let mut status_total = closure
        .iter()
        .filter(|edge| manifest.edges[**edge].rule != "phony")
        .count();
    let has_limited_pool = manifest.has_pool_bindings()
        && closure
            .iter()
            .any(|edge| limited_pool(manifest, &manifest.edges[*edge]).is_some());
    if !progress.prefix_edges.is_empty() || has_limited_pool {
        let dirty = initially_dirty_edges(
            manifest,
            &closure,
            &output_map,
            &build_log,
            &discovered,
            &stat_cache,
            manifest.has_pool_bindings(),
        );
        status_total = progress.offset
            + closure
                .iter()
                .filter(|edge| dirty[**edge] && !progress.completed_edges.contains(edge))
                .filter(|edge| manifest.edges[**edge].rule != "phony")
                .count();
        initially_dirty = Some(dirty);
    }
    let mut status =
        if status_needs_prediction(&options.status_format, options.status_format_explicit) {
            StatusFormatter::with_history(
                options.jobs,
                closure
                    .iter()
                    .filter(|edge| manifest.edges[**edge].rule != "phony")
                    .map(|edge| build_log.previous_elapsed(&manifest.edges[*edge])),
            )
        } else {
            StatusFormatter::new(options.jobs)
        };
    let mut printer = BuildOutput::new(!options.verbose && !options.quiet, options.dry_run);
    let mut finished_count = 0usize;
    if options.dry_run {
        for edge_id in progress.completed_edges.iter().copied() {
            if !in_closure[edge_id] || finished[edge_id] {
                continue;
            }
            ran[edge_id] = true;
            stat_cache.mark_edge(&manifest.edges[edge_id], u128::MAX);
            if finish_edge(
                edge_id,
                true,
                &mut finished,
                &mut failed_prerequisite,
                &dependents,
                &mut pending,
                &mut ready,
                &mut newly_ready,
                &critical_path,
            ) {
                finished_count += 1;
            }
        }
    }

    if !options.quiet {
        for (index, edge_id) in progress.prefix_edges.iter().copied().enumerate() {
            let edge = &manifest.edges[edge_id];
            if status.tracks_prediction() {
                let previous_elapsed = build_log.previous_elapsed(edge);
                status.finish_edge(
                    previous_elapsed,
                    Duration::from_millis(u64::from(previous_elapsed.unwrap_or(0))),
                );
            }
            let command = evaluate_binding(manifest, edge, "command");
            let description = evaluate_binding(manifest, edge, "description");
            let display = if options.verbose || description.is_empty() {
                &command
            } else {
                &description
            };
            let line = status.format(
                &options.status_format,
                options.status_format_explicit,
                StatusSnapshot {
                    started: index + 1,
                    finished: index + 1,
                    running: 1,
                    total: status_total,
                    description: display,
                    elapsed: Duration::ZERO,
                },
            )?;
            printer.print_status(&line, options.verbose)?;
        }
    }
    if options.explain {
        for dyndep in &progress.loaded_dyndeps {
            eprintln!("{} explain: loading dyndep file '{dyndep}'", program_name());
        }
        if program_name() == "ninja" && !progress.loaded_dyndeps.is_empty() {
            for &edge_id in &closure {
                let edge = &manifest.edges[edge_id];
                if edge.rule != "phony"
                    && !evaluate_unescaped_binding(manifest, edge, "dyndep").is_empty()
                    && let Some(output) = edge
                        .outputs()
                        .find(|output| stat_cache.get(output).is_none())
                {
                    // Ninja currently reports this once during dyndep loading
                    // and again during the normal dirty-edge scan.
                    eprintln!("ninja explain: output {output} doesn't exist");
                }
            }
        }
    }

    while finished_count < closure.len() {
        let mut made_progress = false;
        if let Some(dirty) = initially_dirty.as_deref() {
            reserve_new_pool_edges(
                manifest,
                &mut newly_ready,
                &mut pool_reserved,
                &mut pool_usage,
                dirty,
            );
            let completed = finish_ready_clean_phonies(
                manifest,
                dirty,
                &mut finished,
                &mut failed_prerequisite,
                &dependents,
                &mut pending,
                &mut ready,
                &mut newly_ready,
                &critical_path,
            );
            finished_count += completed;
            made_progress |= completed != 0;
            reserve_new_pool_edges(
                manifest,
                &mut newly_ready,
                &mut pool_reserved,
                &mut pool_usage,
                dirty,
            );
            admit_pool_edges(
                manifest,
                &mut ready,
                &mut pool_waiting,
                &mut pool_reserved,
                &mut pool_usage,
                dirty,
            );
        }
        let mut ready_count = if options.dry_run {
            usize::MAX
        } else {
            ready.len()
        };
        let mut ready_examined = 0usize;
        while ready_examined < ready_count {
            if !newly_ready.is_empty()
                && let Some(dirty) = initially_dirty.as_deref()
            {
                reserve_new_pool_edges(
                    manifest,
                    &mut newly_ready,
                    &mut pool_reserved,
                    &mut pool_usage,
                    dirty,
                );
                admit_pool_edges(
                    manifest,
                    &mut ready,
                    &mut pool_waiting,
                    &mut pool_reserved,
                    &mut pool_usage,
                    dirty,
                );
            }
            let Some((_, Reverse(edge_id))) = ready.pop() else {
                break;
            };
            ready_examined += 1;
            if finished[edge_id] {
                continue;
            }
            if failed_prerequisite[edge_id] {
                release_pool_edge(manifest, edge_id, &mut pool_reserved, &mut pool_usage);
                if finish_edge(
                    edge_id,
                    false,
                    &mut finished,
                    &mut failed_prerequisite,
                    &dependents,
                    &mut pending,
                    &mut ready,
                    &mut newly_ready,
                    &critical_path,
                ) {
                    finished_count += 1;
                }
                made_progress = true;
                continue;
            }
            let edge = &manifest.edges[edge_id];
            if edge.rule == "phony" {
                for input in edge.explicit_inputs.iter().chain(&edge.implicit_inputs) {
                    if !output_map.contains_key(input.as_str())
                        && stat_cache.checked_get(input)?.is_none()
                    {
                        return Err(format!("input '{input}' is missing"));
                    }
                }
                release_pool_edge(manifest, edge_id, &mut pool_reserved, &mut pool_usage);
                if finish_edge(
                    edge_id,
                    true,
                    &mut finished,
                    &mut failed_prerequisite,
                    &dependents,
                    &mut pending,
                    &mut ready,
                    &mut newly_ready,
                    &critical_path,
                ) {
                    finished_count += 1;
                }
                made_progress = true;
                continue;
            }

            let evaluated = evaluate_edge(
                edge_id,
                edge,
                &mut EvaluationContext {
                    manifest,
                    output_map: &output_map,
                    build_log: &build_log,
                    stat_cache: &mut stat_cache,
                    discovered: &discovered,
                    ran: &ran,
                    restat_cleaned_outputs: &restat_cleaned_outputs,
                },
                &mut command_buffer,
            )?;
            if !evaluated.dirty {
                outcome.edges_clean += 1;
                if initially_dirty.as_ref().is_none_or(|dirty| dirty[edge_id])
                    && !progress.completed_edges.contains(&edge_id)
                {
                    status_total = status_total.saturating_sub(1);
                    if status.tracks_prediction() {
                        status.remove_edge(build_log.previous_elapsed(edge));
                    }
                }
                release_pool_edge(manifest, edge_id, &mut pool_reserved, &mut pool_usage);
                if finish_edge(
                    edge_id,
                    true,
                    &mut finished,
                    &mut failed_prerequisite,
                    &dependents,
                    &mut pending,
                    &mut ready,
                    &mut newly_ready,
                    &critical_path,
                ) {
                    finished_count += 1;
                }
                made_progress = true;
                continue;
            }
            if options.explain {
                eprintln!("{} explain: {}", program_name(), evaluated.reason);
            }
            if initially_dirty.is_none() {
                let dirty = initially_dirty_edges(
                    manifest,
                    &closure,
                    &output_map,
                    &build_log,
                    &discovered,
                    &stat_cache,
                    manifest.has_pool_bindings(),
                );
                status_total = progress.offset
                    + closure
                        .iter()
                        .filter(|edge| dirty[**edge] && !progress.completed_edges.contains(edge))
                        .filter(|edge| manifest.edges[**edge].rule != "phony")
                        .count();
                initially_dirty = Some(dirty);
            }
            if let Some((pool, _)) = limited_pool(manifest, edge)
                && !pool_reserved[edge_id]
            {
                pool_waiting
                    .entry(pool)
                    .or_default()
                    .push((critical_path[edge_id], Reverse(edge_id)));
                admit_pool_edges(
                    manifest,
                    &mut ready,
                    &mut pool_waiting,
                    &mut pool_reserved,
                    &mut pool_usage,
                    initially_dirty
                        .as_deref()
                        .expect("dirty plan initialized before pool admission"),
                );
                if !options.dry_run && pool_reserved[edge_id] {
                    ready_count += 1;
                }
                made_progress = true;
                continue;
            }
            if options.dry_run {
                commands_started += 1;
                if !options.quiet {
                    let display = if options.verbose || evaluated.description.is_empty() {
                        &evaluated.command
                    } else {
                        &evaluated.description
                    };
                    dry_run_statuses.push((edge_id, display.clone(), start.elapsed()));
                }
                outcome.commands_run += 1;
                outcome.ran_edges.push(edge_id);
                ran[edge_id] = true;
                dry_run_pending.push_back(edge_id);
                made_progress = true;
                continue;
            }
            if stop_starting || !can_run_more(running, options) {
                ready.push((critical_path[edge_id], Reverse(edge_id)));
                continue;
            }
            let pool = evaluated.pool.clone();
            let is_console = pool.as_deref() == Some("console");
            if let Some(client) = &options.jobserver {
                let slot = if implicit_job_slot {
                    implicit_job_slot = false;
                    Some(JobSlot::Implicit)
                } else {
                    client
                        .try_acquire()
                        .map_err(|error| format!("acquiring jobserver token: {error}"))?
                        .map(|token| JobSlot::Explicit { _token: token })
                };
                let Some(slot) = slot else {
                    ready.push((critical_path[edge_id], Reverse(edge_id)));
                    continue;
                };
                job_slots.insert(edge_id, slot);
            }

            commands_started += 1;
            let display = if options.verbose || evaluated.description.is_empty() {
                evaluated.command.clone()
            } else {
                evaluated.description.clone()
            };
            if !options.quiet && (is_console || printer.is_smart_terminal()) {
                let line = status.format(
                    &options.status_format,
                    options.status_format_explicit,
                    StatusSnapshot {
                        started: commands_started,
                        finished: progress.offset + outcome.commands_run + outcome.commands_failed,
                        running: running + 1,
                        total: status_total,
                        description: &display,
                        elapsed: start.elapsed(),
                    },
                )?;
                printer.print_status(&line, options.verbose)?;
                if is_console {
                    printer.finish_line()?;
                }
            }
            if is_console {
                printer.suspend();
            }
            for output in edge.outputs() {
                create_parent_directory(Path::new(output))?;
            }
            let depfile = evaluate_unescaped_binding(manifest, edge, "depfile");
            if !depfile.is_empty() {
                create_parent_directory(Path::new(&depfile))?;
            }
            if let Some(rspfile) = &evaluated.rspfile {
                if let Some(parent) = rspfile.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!(
                            "creating response-file directory '{}': {error}",
                            parent.display()
                        )
                    })?;
                }
                let contents = evaluated.rspfile_content.as_deref().unwrap_or_default();
                #[cfg(windows)]
                let contents = contents.replace('\n', "\r\n");
                fs::write(rspfile, contents)
                    .map_err(|error| format!("writing '{}': {error}", rspfile.display()))?;
            }

            let tx = tx.clone();
            let prior_output_mtimes: Vec<Option<u128>> = edge
                .outputs()
                .map(|output| stat_cache.get(output))
                .collect();
            let command = evaluated.command;
            let log_command = evaluated.log_command;
            let rspfile = evaluated.rspfile;
            let newest_input = evaluated.newest_input;
            let start_ms = start.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
            if let Some(parent) = lock_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("creating build directory: {error}"))?;
            }
            fs::write(&lock_path, b"")
                .map_err(|error| format!("updating '{}': {error}", lock_path.display()))?;
            // Ninja expects the lock-file timestamp taken immediately before
            // launch to be no older than every already-finished input. Some
            // Windows filesystems expose the write slightly late, so preserve
            // that invariant explicitly.
            let start_mtime = modified_ns(&lock_path).unwrap_or(0).max(newest_input);
            register_active_cleanup(
                edge_id,
                edge.outputs()
                    .zip(&prior_output_mtimes)
                    .map(|(output, mtime)| (PathBuf::from(output), *mtime))
                    .collect(),
                (!depfile.is_empty()).then(|| PathBuf::from(&depfile)),
            );
            let jobserver = options.jobserver.clone();
            thread::spawn(move || {
                let output = execute_command(&command, is_console, jobserver.as_ref());
                let end_ms = start.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
                let _ = tx.send(Completion {
                    edge: edge_id,
                    command,
                    log_command,
                    display,
                    rspfile,
                    output,
                    start_ms,
                    end_ms,
                    start_mtime,
                    prior_output_mtimes,
                });
            });
            running += 1;
            made_progress = true;
        }

        if options.dry_run {
            if let Some(edge_id) = dry_run_pending.pop_front() {
                release_pool_edge(manifest, edge_id, &mut pool_reserved, &mut pool_usage);
                stat_cache.mark_edge(&manifest.edges[edge_id], u128::MAX);
                if finish_edge(
                    edge_id,
                    true,
                    &mut finished,
                    &mut failed_prerequisite,
                    &dependents,
                    &mut pending,
                    &mut ready,
                    &mut newly_ready,
                    &critical_path,
                ) {
                    finished_count += 1;
                }
                made_progress = true;
            }
            if !made_progress {
                return Err(format!(
                    "internal scheduler deadlock (ready={}, dry_pending={}, pool_waiting={pool_waiting:?}, pool_usage={pool_usage:?}, reserved={:?})",
                    ready.len(),
                    dry_run_pending.len(),
                    pool_reserved
                        .iter()
                        .enumerate()
                        .filter_map(|(edge, reserved)| reserved.then_some(edge))
                        .collect::<Vec<_>>(),
                ));
            }
            continue;
        }

        if running == 0 {
            if stop_starting {
                break;
            }
            if !made_progress {
                return Err(format!(
                    "internal scheduler deadlock (ready={}, running=0, pool_waiting={}, pool_usage={pool_usage:?})",
                    ready.len(),
                    pool_waiting.values().map(BinaryHeap::len).sum::<usize>(),
                ));
            }
            continue;
        }

        let completion = rx
            .recv()
            .map_err(|_| "command worker terminated unexpectedly")?;
        running -= 1;
        let edge = &manifest.edges[completion.edge];
        if let Some(slot) = job_slots.remove(&completion.edge)
            && matches!(slot, JobSlot::Implicit)
        {
            implicit_job_slot = true;
        }
        let pool = edge_pool(manifest, edge);
        let was_console = pool.as_deref() == Some("console");
        if was_console {
            printer.resume()?;
        }
        release_pool_edge(
            manifest,
            completion.edge,
            &mut pool_reserved,
            &mut pool_usage,
        );
        let mut output = completion
            .output
            .map_err(|error| format!("starting command '{}': {error}", completion.command))?;
        #[cfg(unix)]
        if output.status.code() == Some(130) {
            LAST_BUILD_EXIT_CODE.with(|code| code.set(130));
            terminate_active_process_groups();
            cleanup_interrupted_outputs();
            return Err("build stopped: interrupted by user.".to_owned());
        }
        unregister_active_cleanup(completion.edge);
        let dependency_result =
            extract_dependencies(manifest, edge, &mut output, options.keep_depfile);
        if status.tracks_prediction() {
            status.finish_edge(
                build_log.previous_elapsed(edge),
                Duration::from_millis(u64::from(
                    completion.end_ms.saturating_sub(completion.start_ms),
                )),
            );
        }
        if !options.quiet && !was_console {
            let line = status.format(
                &options.status_format,
                options.status_format_explicit,
                StatusSnapshot {
                    started: commands_started,
                    finished: progress.offset + outcome.commands_run + outcome.commands_failed + 1,
                    running: running + 1,
                    total: status_total,
                    description: &completion.display,
                    elapsed: start.elapsed(),
                },
            )?;
            printer.print_status(&line, options.verbose)?;
        }
        let succeeded = output.status.success() && dependency_result.is_ok();
        if succeeded {
            printer.write_command_output(&output.stdout)?;
            printer.write_command_output(&output.stderr)?;
            if !options.keep_rsp
                && let Some(rspfile) = &completion.rspfile
            {
                let _ = fs::remove_file(rspfile);
            }
            let restat = truthy(&evaluate_binding(manifest, edge, "restat"));
            let generator = truthy(&evaluate_binding(manifest, edge, "generator"));
            let current_output_mtimes = edge
                .outputs()
                .map(|output| {
                    if completion.start_mtime == 0 || restat || generator {
                        checked_modified_ns(Path::new(output))
                            .map_err(|error| format!("build stopped: {error}"))
                    } else {
                        Ok(modified_ns(Path::new(output)))
                    }
                })
                .collect::<Result<Vec<_>, String>>()?;
            let previous_log_mtimes = edge
                .outputs()
                .map(|output| build_log.recorded_mtime(output))
                .collect::<Vec<_>>();
            let restat_cleaned = restat
                && completion
                    .prior_output_mtimes
                    .iter()
                    .zip(&current_output_mtimes)
                    .any(|(before, after)| before == after);
            if restat {
                for ((output, before), after) in edge
                    .outputs()
                    .zip(&completion.prior_output_mtimes)
                    .zip(&current_output_mtimes)
                {
                    if before == after {
                        restat_cleaned_outputs.insert(output);
                    }
                }
            }
            let mut record_mtime = completion.start_mtime;
            if completion.start_mtime == 0 || restat || generator {
                record_mtime = current_output_mtimes
                    .iter()
                    .flatten()
                    .copied()
                    .max()
                    .unwrap_or(completion.start_mtime);
                if restat_cleaned {
                    record_mtime = completion.start_mtime;
                }
            }
            build_log
                .record(
                    edge,
                    &completion.log_command,
                    completion.start_ms,
                    completion.end_ms,
                    record_mtime,
                )
                .map_err(|error| format!("writing build log: {error}"))?;
            for (((output, before), after), previous_log_mtime) in edge
                .outputs()
                .zip(&completion.prior_output_mtimes)
                .zip(&current_output_mtimes)
                .zip(previous_log_mtimes)
            {
                let mtime = if restat && before.is_none() && after.is_none() {
                    Some(u128::from(previous_log_mtime.unwrap_or(0)))
                } else {
                    // Ninja permits successful commands that intentionally do
                    // not materialize their outputs. Treat such outputs as
                    // freshly produced for the rest of this invocation so
                    // their dependents can still run; the next invocation
                    // stats the path again and rebuilds it.
                    after.or(Some(u128::MAX))
                };
                stat_cache.mtimes.insert(output, mtime);
            }
            if let Some(inputs) = dependency_result.unwrap() {
                discovered.record(completion.edge, edge, inputs)?;
            }
            ran[completion.edge] = true;
            outcome.commands_run += 1;
            outcome.ran_edges.push(completion.edge);
        } else {
            let exit_code = output.status.code().unwrap_or(1).clamp(1, 255) as u8;
            LAST_BUILD_EXIT_CODE.with(|code| code.set(exit_code));
            let failed = format!("FAILED: [code={exit_code}] {} ", edge_label(edge));
            if printer.supports_color() {
                printer.print_on_new_line(format!("\x1b[31m{failed}\x1b[0m\n").as_bytes())?;
            } else {
                printer.print_on_new_line(format!("{failed}\n").as_bytes())?;
            }
            printer.print_on_new_line(format!("{}\n", completion.command).as_bytes())?;
            printer.write_command_output(&output.stdout)?;
            printer.write_command_output(&output.stderr)?;
            if let Err(error) = &dependency_result
                && output.status.success()
            {
                printer.print_on_new_line(format!("{error}\n").as_bytes())?;
            }
            let failure = if output.status.success() {
                dependency_result.unwrap_err()
            } else {
                format!("{} failed with {}", edge_label(edge), output.status)
            };
            failures.push(failure);
            outcome.commands_failed += 1;
            let limit = options.failures_allowed;
            if limit != 0 && outcome.commands_failed >= limit {
                stop_starting = true;
            }
        }
        if finish_edge(
            completion.edge,
            succeeded,
            &mut finished,
            &mut failed_prerequisite,
            &dependents,
            &mut pending,
            &mut ready,
            &mut newly_ready,
            &critical_path,
        ) {
            finished_count += 1;
        }
    }

    for (index, (edge_id, display, elapsed)) in dry_run_statuses.into_iter().enumerate() {
        if printer.is_smart_terminal() {
            let line = status.format(
                &options.status_format,
                options.status_format_explicit,
                StatusSnapshot {
                    started: progress.offset + index + 1,
                    finished: progress.offset + index,
                    running: 1,
                    total: status_total,
                    description: &display,
                    elapsed,
                },
            )?;
            printer.print_status(&line, options.verbose)?;
        }
        if status.tracks_prediction() {
            status.finish_edge(
                build_log.previous_elapsed(&manifest.edges[edge_id]),
                Duration::ZERO,
            );
        }
        let line = status.format(
            &options.status_format,
            options.status_format_explicit,
            StatusSnapshot {
                started: progress.offset + index + 1,
                finished: progress.offset + index + 1,
                running: 1,
                total: status_total,
                description: &display,
                elapsed,
            },
        )?;
        printer.print_status(&line, options.verbose)?;
    }

    printer.finish_line()?;

    if options.stats {
        print_build_stats(
            preparation,
            scheduler_setup_time,
            preparation.build_log,
            stat_time,
            start.elapsed(),
            closure.len(),
        );
    }

    if !failures.is_empty() {
        if !stop_starting {
            return Err(format!(
                "build stopped: cannot make progress due to previous errors\n{} subcommand(s) failed\n{}",
                failures.len(),
                failures.join("\n")
            ));
        }
        return Err(format!(
            "build stopped: {} subcommand(s) failed\n{}",
            failures.len(),
            failures.join("\n")
        ));
    }
    if outcome.commands_run == 0 && !options.quiet_no_work && !options.quiet {
        printer.print_on_new_line(format!("{}: no work to do.\n", program_name()).as_bytes())?;
        printer.finish_line()?;
    }
    Ok(outcome)
}

fn initially_dirty_edges(
    manifest: &Manifest,
    closure: &[usize],
    output_map: &HashMap<&str, usize>,
    build_log: &BuildLog<'_>,
    discovered: &DiscoveredDeps,
    stat_cache: &StatCache<'_>,
    track_phony: bool,
) -> Vec<bool> {
    let mut dirty = vec![false; manifest.edges.len()];
    let mut stat_cache = stat_cache.clone();
    let mut command_buffer = String::new();
    let restat_cleaned_outputs = HashSet::new();
    for &edge_id in closure {
        let edge = &manifest.edges[edge_id];
        if edge.rule == "phony" {
            if track_phony {
                dirty[edge_id] = edge
                    .explicit_inputs
                    .iter()
                    .chain(&edge.implicit_inputs)
                    .any(|input| {
                        output_map
                            .get(input.as_str())
                            .is_some_and(|producer| dirty[*producer])
                            || (!output_map.contains_key(input.as_str())
                                && stat_cache.get(input).is_none())
                    });
            }
            continue;
        }
        dirty[edge_id] = evaluate_edge(
            edge_id,
            edge,
            &mut EvaluationContext {
                manifest,
                output_map,
                build_log,
                stat_cache: &mut stat_cache,
                discovered,
                ran: &dirty,
                restat_cleaned_outputs: &restat_cleaned_outputs,
            },
            &mut command_buffer,
        )
        .map_or(true, |evaluated| evaluated.dirty);
        if dirty[edge_id] {
            stat_cache.mark_edge(edge, u128::MAX);
        }
    }
    dirty
}

fn ensure_build_directory(manifest: &Manifest, dry_run: bool) -> Result<(), String> {
    if dry_run {
        return Ok(());
    }
    let Some(builddir) = manifest
        .variables
        .get("builddir")
        .filter(|builddir| !builddir.is_empty())
    else {
        return Ok(());
    };
    fs::create_dir_all(builddir)
        .map_err(|error| format!("creating build directory '{builddir}': {error}"))
}

#[derive(Debug)]
struct StatusFormatter {
    rate_window: usize,
    rate_times: VecDeque<(usize, Duration)>,
    prediction: Option<ProgressPrediction>,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProgressPrediction {
    total_edges: usize,
    finished_edges: usize,
    cpu_time_ms: u64,
    predictable_edges_total: usize,
    predictable_edges_remaining: usize,
    predictable_cpu_time_total_ms: u64,
    predictable_cpu_time_remaining_ms: u64,
    unpredictable_edges_remaining: usize,
}

#[derive(Clone, Copy, Debug)]
struct StatusSnapshot<'a> {
    started: usize,
    finished: usize,
    running: usize,
    total: usize,
    description: &'a str,
    elapsed: Duration,
}

impl StatusFormatter {
    fn new(jobs: usize) -> Self {
        Self {
            rate_window: jobs.clamp(1, 4096),
            rate_times: VecDeque::new(),
            prediction: None,
        }
    }

    fn with_history(jobs: usize, previous_elapsed: impl IntoIterator<Item = Option<u32>>) -> Self {
        let mut status = Self::new(jobs);
        let mut prediction = ProgressPrediction::default();
        for elapsed in previous_elapsed {
            prediction.add_edge(elapsed);
        }
        status.prediction = Some(prediction);
        status
    }

    fn tracks_prediction(&self) -> bool {
        self.prediction.is_some()
    }

    fn remove_edge(&mut self, previous_elapsed: Option<u32>) {
        if let Some(prediction) = &mut self.prediction {
            prediction.remove_edge(previous_elapsed);
        }
    }

    fn finish_edge(&mut self, previous_elapsed: Option<u32>, elapsed: Duration) {
        if let Some(prediction) = &mut self.prediction {
            prediction.finish_edge(previous_elapsed, elapsed);
        }
    }

    fn format(
        &mut self,
        format: &str,
        explicit: bool,
        snapshot: StatusSnapshot<'_>,
    ) -> Result<String, String> {
        let StatusSnapshot {
            started,
            finished,
            running,
            total,
            description,
            elapsed,
        } = snapshot;
        if self
            .rate_times
            .back()
            .is_none_or(|(last, _)| *last != finished)
        {
            if self.rate_times.len() == self.rate_window {
                self.rate_times.pop_front();
            }
            self.rate_times.push_back((finished, elapsed));
        }

        let remaining = total.saturating_sub(started);
        let progress = percentage(finished, total);
        let elapsed_seconds = elapsed.as_secs_f64();
        let overall_rate = if finished == 0 || elapsed_seconds == 0.0 {
            None
        } else {
            Some(finished as f64 / elapsed_seconds)
        };
        let current_rate = self
            .rate_times
            .front()
            .zip(self.rate_times.back())
            .and_then(|((first_count, first_time), (last_count, last_time))| {
                let seconds = last_time.saturating_sub(*first_time).as_secs_f64();
                (seconds > 0.0).then(|| {
                    last_count.saturating_sub(*first_count).saturating_add(1) as f64 / seconds
                })
            });
        let predicted_progress = self
            .prediction
            .map(|prediction| prediction.percentage(elapsed))
            .unwrap_or_else(|| finished as f64 / total.max(1) as f64);
        let eta_seconds = (predicted_progress > 0.0)
            .then(|| (elapsed_seconds / predicted_progress - elapsed_seconds).max(0.0));
        let print_hours =
            elapsed_seconds >= 3600.0 || eta_seconds.is_some_and(|seconds| seconds >= 3600.0);

        let value = |name: &str| -> Result<String, String> {
            Ok(match name {
                "started" => started.to_string(),
                "finished" => finished.to_string(),
                "running" => running.to_string(),
                "remaining" => remaining.to_string(),
                "total" => total.to_string(),
                "progress" => format!("{progress:3}%"),
                "rate" => format_rate(overall_rate),
                "current_rate" => format_rate(current_rate),
                "elapsed_seconds" => format!("{elapsed_seconds:.3}"),
                "eta_seconds" => format_seconds(eta_seconds),
                "elapsed" => format_clock(Some(elapsed_seconds), print_hours),
                "eta" => format_clock(eta_seconds, print_hours),
                "predicted_progress" => {
                    format!("{:3}%", (100.0 * predicted_progress) as usize)
                }
                "description" => description.to_owned(),
                _ => return Err(format!("unknown variable '{name}' in --status format")),
            })
        };

        let rendered = if explicit {
            expand_status_variables(format, value)?
        } else {
            let mut rendered = expand_status_placeholders(
                format,
                started,
                finished,
                running,
                total,
                overall_rate,
                current_rate,
                elapsed_seconds,
                eta_seconds,
                progress,
                (100.0 * predicted_progress) as usize,
                print_hours,
            )?;
            rendered.push_str(description);
            rendered
        };
        Ok(rendered)
    }
}

fn status_needs_prediction(format: &str, explicit: bool) -> bool {
    if explicit {
        return format.contains("$predicted_progress") || format.contains("$eta");
    }
    let mut chars = format.chars();
    while let Some(character) = chars.next() {
        if character == '%'
            && chars
                .next()
                .is_some_and(|placeholder| matches!(placeholder, 'P' | 'E' | 'W'))
        {
            return true;
        }
    }
    false
}

impl ProgressPrediction {
    fn add_edge(&mut self, previous_elapsed: Option<u32>) {
        self.total_edges += 1;
        if let Some(elapsed) = previous_elapsed {
            self.predictable_edges_total += 1;
            self.predictable_edges_remaining += 1;
            self.predictable_cpu_time_total_ms += u64::from(elapsed);
            self.predictable_cpu_time_remaining_ms += u64::from(elapsed);
        } else {
            self.unpredictable_edges_remaining += 1;
        }
    }

    fn remove_edge(&mut self, previous_elapsed: Option<u32>) {
        self.total_edges = self.total_edges.saturating_sub(1);
        if let Some(elapsed) = previous_elapsed {
            self.predictable_edges_total = self.predictable_edges_total.saturating_sub(1);
            self.predictable_edges_remaining = self.predictable_edges_remaining.saturating_sub(1);
            self.predictable_cpu_time_total_ms = self
                .predictable_cpu_time_total_ms
                .saturating_sub(u64::from(elapsed));
            self.predictable_cpu_time_remaining_ms = self
                .predictable_cpu_time_remaining_ms
                .saturating_sub(u64::from(elapsed));
        } else {
            self.unpredictable_edges_remaining =
                self.unpredictable_edges_remaining.saturating_sub(1);
        }
    }

    fn finish_edge(&mut self, previous_elapsed: Option<u32>, elapsed: Duration) {
        self.finished_edges += 1;
        self.cpu_time_ms = self
            .cpu_time_ms
            .saturating_add(elapsed.as_millis().min(u128::from(u64::MAX)) as u64);
        if let Some(previous) = previous_elapsed {
            self.predictable_edges_remaining = self.predictable_edges_remaining.saturating_sub(1);
            self.predictable_cpu_time_remaining_ms = self
                .predictable_cpu_time_remaining_ms
                .saturating_sub(u64::from(previous));
        } else {
            self.unpredictable_edges_remaining =
                self.unpredictable_edges_remaining.saturating_sub(1);
        }
    }

    fn percentage(self, wall_time: Duration) -> f64 {
        let mut use_previous =
            self.predictable_edges_remaining != 0 && self.predictable_cpu_time_remaining_ms != 0;
        if use_previous
            && self.total_edges != 0
            && self.finished_edges != 0
            && wall_time >= Duration::from_secs(15)
            && self.finished_edges as f64 / self.total_edges as f64 >= 0.05
        {
            let actual_average = self.cpu_time_ms as f64 / self.finished_edges as f64;
            let previous_average =
                self.predictable_cpu_time_total_ms as f64 / self.predictable_edges_total as f64;
            let ratio = actual_average.max(previous_average) / actual_average.min(previous_average);
            use_previous = ratio < 10.0;
        }

        let known_edges = self.finished_edges
            + if use_previous {
                self.predictable_edges_remaining
            } else {
                0
            };
        if known_edges == 0 {
            return 0.0;
        }
        let unknown_edges = if use_previous {
            self.unpredictable_edges_remaining
        } else {
            self.total_edges.saturating_sub(self.finished_edges)
        };
        let known_cpu_ms = self.cpu_time_ms
            + if use_previous {
                self.predictable_cpu_time_remaining_ms
            } else {
                0
            };
        let average_cpu_ms = known_cpu_ms as f64 / known_edges as f64;
        let remaining_cpu_ms = average_cpu_ms * unknown_edges as f64
            + if use_previous {
                self.predictable_cpu_time_remaining_ms as f64
            } else {
                0.0
            };
        let total_cpu_ms = self.cpu_time_ms as f64 + remaining_cpu_ms;
        if total_cpu_ms == 0.0 {
            0.0
        } else {
            self.cpu_time_ms as f64 / total_cpu_ms
        }
    }
}

fn percentage(finished: usize, total: usize) -> usize {
    if finished == 0 || total == 0 {
        0
    } else {
        finished.saturating_mul(100) / total
    }
}

fn format_rate(rate: Option<f64>) -> String {
    rate.map_or_else(|| "?".to_owned(), |rate| format!("{rate:.1}"))
}

fn format_seconds(seconds: Option<f64>) -> String {
    seconds.map_or_else(|| "?".to_owned(), |seconds| format!("{seconds:.3}"))
}

fn format_clock(seconds: Option<f64>, print_hours: bool) -> String {
    let Some(seconds) = seconds else {
        return "?".to_owned();
    };
    let seconds = seconds as u64;
    if print_hours {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds % 3600 / 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_status_placeholders(
    format: &str,
    started: usize,
    finished: usize,
    running: usize,
    total: usize,
    overall_rate: Option<f64>,
    current_rate: Option<f64>,
    elapsed_seconds: f64,
    eta_seconds: Option<f64>,
    progress: usize,
    predicted_progress: usize,
    print_hours: bool,
) -> Result<String, String> {
    let remaining = total.saturating_sub(started);
    let mut rendered = String::with_capacity(format.len() + 16);
    let mut chars = format.chars();
    while let Some(character) = chars.next() {
        if character != '%' {
            rendered.push(character);
            continue;
        }
        let placeholder = chars
            .next()
            .ok_or_else(|| "unknown placeholder '%' in NINJA_STATUS".to_owned())?;
        let value = match placeholder {
            '%' => "%".to_owned(),
            's' => started.to_string(),
            'f' => finished.to_string(),
            'r' => running.to_string(),
            'u' => remaining.to_string(),
            't' => total.to_string(),
            'p' => format!("{progress:3}%"),
            'o' => format_rate(overall_rate),
            'c' => format_rate(current_rate),
            'e' => format!("{elapsed_seconds:.3}"),
            'E' => format_seconds(eta_seconds),
            'w' => format_clock(Some(elapsed_seconds), print_hours),
            'W' => format_clock(eta_seconds, print_hours),
            'P' => format!("{predicted_progress:3}%"),
            other => return Err(format!("unknown placeholder '%{other}' in NINJA_STATUS")),
        };
        rendered.push_str(&value);
    }
    Ok(rendered)
}

fn expand_status_variables(
    format: &str,
    mut value: impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut rendered = String::with_capacity(format.len() + 16);
    let mut chars = format.char_indices().peekable();
    while let Some((_, character)) = chars.next() {
        if character != '$' {
            rendered.push(character);
            continue;
        }
        let Some(&(next_index, next)) = chars.peek() else {
            return Err("bad $-escape (literal $ must be written as $$)".to_owned());
        };
        if next == '$' {
            chars.next();
            rendered.push('$');
            continue;
        }
        if matches!(next, ' ' | ':') {
            chars.next();
            rendered.push(next);
            continue;
        }
        let name = if next == '{' {
            chars.next();
            let start = next_index + 1;
            let mut end = None;
            for (index, character) in chars.by_ref() {
                if character == '}' {
                    end = Some(index);
                    break;
                }
            }
            let end = end.ok_or_else(|| "bad $-escape (missing '}')".to_owned())?;
            &format[start..end]
        } else {
            let start = next_index;
            let mut end = format.len();
            while let Some(&(index, character)) = chars.peek() {
                if !character.is_ascii_alphanumeric() && character != '_' {
                    end = index;
                    break;
                }
                chars.next();
            }
            if end == start {
                return Err("bad $-escape (literal $ must be written as $$)".to_owned());
            }
            &format[start..end]
        };
        rendered.push_str(&value(name)?);
    }
    Ok(rendered)
}

fn create_parent_directory(path: &Path) -> Result<(), String> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    fs::create_dir_all(parent)
        .map_err(|error| format!("creating output directory '{}': {error}", parent.display()))
}

struct Dependents {
    heads: Vec<usize>,
    tails: Vec<usize>,
    edges: Vec<usize>,
    next: Vec<usize>,
}

impl Dependents {
    const END: usize = usize::MAX;

    fn new(edge_count: usize, expected_links: usize, preserve_order: bool) -> Self {
        Self {
            heads: vec![Self::END; edge_count],
            tails: if preserve_order {
                vec![Self::END; edge_count]
            } else {
                Vec::new()
            },
            edges: Vec::with_capacity(expected_links),
            next: Vec::with_capacity(expected_links),
        }
    }

    fn add(&mut self, prerequisite: usize, dependent: usize) {
        if self.tails.is_empty() {
            self.edges.push(dependent);
            self.next.push(self.heads[prerequisite]);
            self.heads[prerequisite] = self.edges.len() - 1;
            return;
        }
        let link = self.edges.len();
        self.edges.push(dependent);
        self.next.push(Self::END);
        if self.heads[prerequisite] == Self::END {
            self.heads[prerequisite] = link;
        } else {
            self.next[self.tails[prerequisite]] = link;
        }
        self.tails[prerequisite] = link;
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_edge(
    edge: usize,
    succeeded: bool,
    finished: &mut [bool],
    failed_prerequisite: &mut [bool],
    dependents: &Dependents,
    pending: &mut [usize],
    ready: &mut BinaryHeap<(usize, Reverse<usize>)>,
    newly_ready: &mut Vec<usize>,
    critical_path: &[usize],
) -> bool {
    if finished[edge] {
        return false;
    }
    finished[edge] = true;
    let mut link = dependents.heads[edge];
    while link != Dependents::END {
        let dependent = dependents.edges[link];
        if !succeeded {
            failed_prerequisite[dependent] = true;
        }
        pending[dependent] -= 1;
        if pending[dependent] == 0 {
            ready.push((critical_path[dependent], Reverse(dependent)));
            newly_ready.push(dependent);
        }
        link = dependents.next[link];
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn finish_ready_clean_phonies(
    manifest: &Manifest,
    initially_dirty: &[bool],
    finished: &mut [bool],
    failed_prerequisite: &mut [bool],
    dependents: &Dependents,
    pending: &mut [usize],
    ready: &mut BinaryHeap<(usize, Reverse<usize>)>,
    newly_ready: &mut Vec<usize>,
    critical_path: &[usize],
) -> usize {
    fn classify(
        candidate: (usize, Reverse<usize>),
        manifest: &Manifest,
        initially_dirty: &[bool],
        failed_prerequisite: &[bool],
        retained: &mut Vec<(usize, Reverse<usize>)>,
        phonies: &mut VecDeque<usize>,
    ) {
        let edge_id = candidate.1.0;
        let edge = &manifest.edges[edge_id];
        let inputless = edge.inputs().next().is_none();
        if edge.rule == "phony"
            && !failed_prerequisite[edge_id]
            && (!initially_dirty[edge_id] || inputless)
        {
            phonies.push_back(edge_id);
        } else {
            retained.push(candidate);
        }
    }

    let mut retained = Vec::new();
    let mut phonies = VecDeque::new();
    while let Some(candidate) = ready.pop() {
        classify(
            candidate,
            manifest,
            initially_dirty,
            failed_prerequisite,
            &mut retained,
            &mut phonies,
        );
    }

    let mut count = 0;
    while let Some(edge_id) = phonies.pop_front() {
        if finish_edge(
            edge_id,
            true,
            finished,
            failed_prerequisite,
            dependents,
            pending,
            ready,
            newly_ready,
            critical_path,
        ) {
            count += 1;
        }
        while let Some(candidate) = ready.pop() {
            classify(
                candidate,
                manifest,
                initially_dirty,
                failed_prerequisite,
                &mut retained,
                &mut phonies,
            );
        }
    }
    ready.extend(retained);
    count
}

fn print_build_stats(
    preparation: PreparationStats,
    scheduler_setup: Duration,
    build_log: Duration,
    stat: Duration,
    execute: Duration,
    edges: usize,
) {
    let row = |name: &str, duration: Duration| {
        eprintln!(
            "{} stats: {name:<20} {:>9.3} ms",
            program_name(),
            duration.as_secs_f64() * 1000.0
        );
    };
    row("output index", preparation.output_map);
    row("dependency metadata", preparation.dependencies);
    row("target closure", preparation.closure);
    row("scheduler graph", scheduler_setup);
    row("build log", build_log);
    row("filesystem stat", stat);
    row("edge evaluation", execute);
    eprintln!("{} stats: edges evaluated       {edges:>9}", program_name());
}

fn output_map(manifest: &Manifest) -> HashMap<&str, usize> {
    let mut result = HashMap::with_capacity(manifest.edges.len() * 2);
    for (id, edge) in manifest.edges.iter().enumerate() {
        for output in edge.outputs() {
            result.insert(output, id);
        }
    }
    result
}

fn edge_label(edge: &Edge) -> String {
    edge.outputs().collect::<Vec<_>>().join(" ")
}

fn select_targets(
    manifest: &Manifest,
    requested: &[String],
    outputs: &HashMap<&str, usize>,
    deps_log: &DepsLog,
) -> Result<Vec<usize>, String> {
    let names = if !requested.is_empty() {
        requested
            .iter()
            .map(|target| {
                let path = canonicalize_path(target);
                if !path.ends_with('^') {
                    if outputs.contains_key(path.as_str()) {
                        return Ok(path);
                    }
                    if let Some(builddir) = manifest.variables.get("builddir")
                        && !builddir.is_empty()
                    {
                        let candidate = canonicalize_path(&format!("{builddir}/{path}"));
                        if outputs.contains_key(candidate.as_str()) {
                            return Ok(candidate);
                        }
                    }
                }
                resolve_target_path(manifest, target, Some(deps_log), true)
            })
            .collect::<Result<Vec<_>, _>>()?
    } else if !manifest.defaults.is_empty() {
        manifest.defaults.clone()
    } else {
        let inputs = manifest
            .edges
            .iter()
            .flat_map(Edge::inputs)
            .collect::<HashSet<_>>();
        let roots = manifest
            .edges
            .iter()
            .filter_map(|edge| {
                edge.outputs()
                    .find(|output| !inputs.contains(output))
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        if roots.is_empty() && !manifest.edges.is_empty() {
            return Err("could not determine root nodes of build graph".to_owned());
        }
        roots
    };
    let mut targets = Vec::new();
    let known_inputs = manifest
        .edges
        .iter()
        .flat_map(|edge| {
            edge.inputs()
                .chain(edge.validations.iter().map(String::as_str))
        })
        .collect::<HashSet<_>>();
    for name in names {
        if let Some(edge) = outputs.get(name.as_str()) {
            targets.push(*edge);
        } else if known_inputs.contains(name.as_str()) {
            if !Path::new(&name).exists() {
                return Err(format!("'{name}' missing and no known rule to make it"));
            }
        } else {
            return Err(unknown_target_message(manifest, &name));
        }
    }
    targets.sort_unstable();
    targets.dedup();
    Ok(targets)
}

pub fn resolve_target_path(
    manifest: &Manifest,
    target: &str,
    deps_log: Option<&DepsLog>,
    use_builddir: bool,
) -> Result<String, String> {
    if target.is_empty() {
        return Err("empty path".to_owned());
    }
    let mut path = canonicalize_path(target);
    let first_dependent = path.ends_with('^');
    if first_dependent {
        path.pop();
    }
    let known = |candidate: &str| {
        manifest.edges.iter().any(|edge| {
            edge.outputs()
                .chain(edge.inputs())
                .chain(edge.validations.iter().map(String::as_str))
                .any(|node| node == candidate)
        })
    };
    let mut is_known = known(&path);
    if use_builddir
        && !is_known
        && let Some(builddir) = manifest.variables.get("builddir")
        && !builddir.is_empty()
    {
        let candidate = canonicalize_path(&format!("{builddir}/{path}"));
        if known(&candidate) {
            path = candidate;
            is_known = true;
        }
    }
    if !is_known {
        return Err(unknown_target_message(manifest, &path));
    }
    if !first_dependent {
        return Ok(path);
    }
    if let Some(output) = manifest.edges.iter().find_map(|edge| {
        edge.inputs()
            .any(|input| input == path)
            .then(|| edge.outputs().next())
            .flatten()
    }) {
        return Ok(output.to_owned());
    }
    if let Some(output) = deps_log.and_then(|log| log.first_reverse_dep(&path)) {
        return Ok(output.to_owned());
    }
    Err(format!("'{path}' has no out edge"))
}

fn dependency_closure(
    manifest: &Manifest,
    targets: &[usize],
    outputs: &HashMap<&str, usize>,
    discovered: &DiscoveredDeps,
    phony_cycle_error: bool,
) -> Result<Vec<usize>, String> {
    #[derive(Clone, Copy)]
    struct Frame {
        edge: usize,
        next_input: usize,
    }

    fn input_at<'a>(edge: &'a Edge, discovered: &'a [String], index: usize) -> Option<&'a str> {
        let mut index = index;
        for inputs in [
            &edge.explicit_inputs,
            &edge.implicit_inputs,
            &edge.order_only_inputs,
            discovered,
        ] {
            if index < inputs.len() {
                return Some(&inputs[index]);
            }
            index -= inputs.len();
        }
        None
    }

    fn visit_iterative(
        root: usize,
        manifest: &Manifest,
        outputs: &HashMap<&str, usize>,
        discovered: &DiscoveredDeps,
        phony_cycle_error: bool,
        state: &mut [u8],
        result: &mut Vec<usize>,
    ) -> Result<(), String> {
        if state[root] == 2 {
            return Ok(());
        }
        if state[root] == 1 {
            return Err(format!(
                "dependency cycle involving '{}'",
                edge_label(&manifest.edges[root])
            ));
        }
        state[root] = 1;
        let mut stack = vec![Frame {
            edge: root,
            next_input: 0,
        }];
        while let Some(frame) = stack.last_mut() {
            let edge_id = frame.edge;
            let edge = &manifest.edges[edge_id];
            let Some(input) = input_at(edge, &discovered.inputs[edge_id], frame.next_input) else {
                state[edge_id] = 2;
                result.push(edge_id);
                stack.pop();
                continue;
            };
            frame.next_input += 1;
            let Some(&producer) = outputs.get(input) else {
                continue;
            };
            if producer == edge_id && tolerates_phony_self_reference(edge, phony_cycle_error) {
                continue;
            }
            match state[producer] {
                0 => {
                    state[producer] = 1;
                    stack.push(Frame {
                        edge: producer,
                        next_input: 0,
                    });
                }
                1 => {
                    let start = stack
                        .iter()
                        .position(|frame| frame.edge == producer)
                        .unwrap_or(0);
                    let mut cycle = vec![input.to_owned()];
                    cycle.extend(
                        stack[start + 1..]
                            .iter()
                            .map(|frame| edge_label(&manifest.edges[frame.edge]).to_owned()),
                    );
                    cycle.push(input.to_owned());
                    return Err(format!("dependency cycle: {}", cycle.join(" -> ")));
                }
                _ => {}
            }
        }
        Ok(())
    }

    let mut state = vec![0u8; manifest.edges.len()];
    let mut result = Vec::new();
    for target in targets {
        visit_iterative(
            *target,
            manifest,
            outputs,
            discovered,
            phony_cycle_error,
            &mut state,
            &mut result,
        )?;
    }
    let mut index = 0;
    while index < result.len() {
        let edge_id = result[index];
        index += 1;
        for validation in &manifest.edges[edge_id].validations {
            if let Some(validation_edge) = outputs.get(validation.as_str()) {
                visit_iterative(
                    *validation_edge,
                    manifest,
                    outputs,
                    discovered,
                    phony_cycle_error,
                    &mut state,
                    &mut result,
                )?;
            }
        }
    }
    Ok(result)
}

fn tolerates_phony_self_reference(edge: &Edge, phony_cycle_error: bool) -> bool {
    !phony_cycle_error
        && edge.rule == "phony"
        && edge.explicit_outputs.len() == 1
        && edge.implicit_outputs.is_empty()
        && edge.implicit_inputs.is_empty()
}

fn extract_dependencies(
    manifest: &Manifest,
    edge: &Edge,
    output: &mut Output,
    keep_depfile: bool,
) -> Result<Option<Vec<String>>, String> {
    let deps_type = evaluate_binding(manifest, edge, "deps");
    match deps_type.as_str() {
        "" => Ok(None),
        "gcc" => {
            let depfile = evaluate_unescaped_binding(manifest, edge, "depfile");
            if depfile.is_empty() {
                return Err("edge with deps=gcc but no depfile makes no sense".to_owned());
            }
            let contents = match fs::read_to_string(&depfile) {
                Ok(contents) => contents,
                Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(format!("reading depfile '{depfile}': {error}")),
            };
            let inputs = if contents.is_empty() {
                Vec::new()
            } else {
                normalize_depfile(
                    parse_depfile(&contents)
                        .map_err(|error| format!("parsing depfile '{depfile}': {error}"))?,
                )
                .inputs
            };
            if !keep_depfile {
                match fs::remove_file(&depfile) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("deleting depfile '{depfile}': {error}")),
                }
            }
            Ok(Some(inputs))
        }
        "msvc" => {
            let prefix = evaluate_binding(manifest, edge, "msvc_deps_prefix");
            let prefix = if prefix.is_empty() {
                "Note: including file: "
            } else {
                &prefix
            };
            let mut inputs = BTreeSet::new();
            output.stdout = filter_msvc_output(&output.stdout, prefix, &mut inputs)?;
            output.stderr = filter_msvc_output(&output.stderr, prefix, &mut inputs)?;
            Ok(Some(inputs.into_iter().collect()))
        }
        unknown => Err(format!("unknown deps type '{unknown}'")),
    }
}

fn normalize_depfile(mut depfile: crate::depfile::Depfile) -> crate::depfile::Depfile {
    for path in depfile.outputs.iter_mut().chain(&mut depfile.inputs) {
        *path = canonicalize_owned_path(std::mem::take(path));
    }
    depfile
}

pub fn filter_msvc_output(
    output: &[u8],
    prefix: &str,
    includes: &mut BTreeSet<String>,
) -> Result<Vec<u8>, String> {
    let output = String::from_utf8_lossy(output);
    let mut filtered = String::with_capacity(output.len());
    let mut saw_include = false;
    for line in output.lines() {
        if let Some(include) = line.strip_prefix(prefix) {
            saw_include = true;
            let include = include.trim_start();
            if let Some(normalized) = normalize_msvc_include(include)? {
                includes.insert(normalized);
            }
        } else if !saw_include && is_compiler_input_echo(line) {
            continue;
        } else {
            filtered.push_str(line);
            filtered.push('\n');
        }
    }
    Ok(filtered.into_bytes())
}

fn is_compiler_input_echo(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [".c", ".cc", ".cxx", ".cpp", ".c++"]
        .iter()
        .any(|extension| lower.ends_with(extension))
}

fn normalize_msvc_include(include: &str) -> Result<Option<String>, String> {
    let lower = include.to_ascii_lowercase();
    if lower.contains("program files") || lower.contains("microsoft visual studio") {
        return Ok(None);
    }
    let path = Path::new(include);
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("resolving include '{include}': {error}"))?
            .join(path)
    };
    let current = std::env::current_dir()
        .map_err(|error| format!("resolving include '{include}': {error}"))?;
    let display = absolute
        .strip_prefix(&current)
        .unwrap_or(&absolute)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(Some(canonicalize_owned_path(display)))
}

struct EvaluatedEdge {
    dirty: bool,
    reason: String,
    command: String,
    log_command: String,
    description: String,
    pool: Option<String>,
    rspfile: Option<PathBuf>,
    rspfile_content: Option<String>,
    newest_input: u128,
}

struct EvaluationContext<'manifest, 'borrow> {
    manifest: &'manifest Manifest,
    output_map: &'borrow HashMap<&'manifest str, usize>,
    build_log: &'borrow BuildLog<'manifest>,
    stat_cache: &'borrow mut StatCache<'manifest>,
    discovered: &'borrow DiscoveredDeps,
    ran: &'borrow [bool],
    restat_cleaned_outputs: &'borrow HashSet<&'manifest str>,
}

fn evaluate_edge(
    edge_id: usize,
    edge: &Edge,
    context: &mut EvaluationContext<'_, '_>,
    command_buffer: &mut String,
) -> Result<EvaluatedEdge, String> {
    let manifest = context.manifest;
    let output_map = context.output_map;
    let stat_cache = &mut *context.stat_cache;
    if let Some(error) = &context.discovered.errors[edge_id] {
        return Err(error.clone());
    }
    command_buffer.clear();
    evaluate_binding_into(manifest, edge, "command", true, command_buffer);
    if command_buffer.trim().is_empty() {
        return Err(format!("rule '{}' has no command", edge.rule));
    }
    let rspfile_content = evaluate_binding(manifest, edge, "rspfile_content");
    let expanded_log_command = if rspfile_content.is_empty() {
        None
    } else {
        Some(format!("{command_buffer};rspfile={rspfile_content}"))
    };
    let log_command = expanded_log_command.as_deref().unwrap_or(command_buffer);
    let generator = truthy(&evaluate_binding(manifest, edge, "generator"));
    let restat = truthy(&evaluate_binding(manifest, edge, "restat"));
    let use_restat = restat
        && edge
            .outputs()
            .all(|output| context.build_log.has_entry(output));
    let mut dirty = false;
    let mut reason = String::new();
    let mut oldest_output = u128::MAX;
    let mut newest_input = 0;
    for output in edge.outputs() {
        if let Some(mtime) = stat_cache.checked_get(output)? {
            oldest_output = oldest_output.min(mtime);
        } else {
            dirty = true;
            reason = format!("output {output} doesn't exist");
            break;
        }
    }
    for input in edge.explicit_inputs.iter().chain(&edge.implicit_inputs) {
        let mtime = virtual_mtime(manifest, input, output_map, &mut HashSet::new(), stat_cache)?;
        newest_input = newest_input.max(mtime);
        let producer_is_dirty = output_map.get(input.as_str()).is_some_and(|producer| {
            context.ran[*producer] && !context.restat_cleaned_outputs.contains(input.as_str())
        });
        if producer_is_dirty || mtime == u128::MAX {
            dirty = true;
            reason = format!("{input} is dirty");
        } else if !dirty && !use_restat && mtime > oldest_output {
            dirty = true;
            reason = format!("input {input} is newer than the oldest output");
        }
    }
    for input in &context.discovered.inputs[edge_id] {
        if !output_map.contains_key(input.as_str()) && stat_cache.checked_get(input)?.is_none() {
            if !dirty {
                dirty = true;
                reason = format!("discovered input '{input}' is missing");
            }
            continue;
        }
        let mtime = virtual_mtime(manifest, input, output_map, &mut HashSet::new(), stat_cache)?;
        newest_input = newest_input.max(mtime);
        let producer_is_dirty = output_map.get(input.as_str()).is_some_and(|producer| {
            context.ran[*producer] && !context.restat_cleaned_outputs.contains(input.as_str())
        });
        if producer_is_dirty || mtime == u128::MAX {
            dirty = true;
            reason = format!("{input} is dirty");
        } else if !dirty && !use_restat && mtime > oldest_output {
            dirty = true;
            reason = format!("discovered input {input} is newer than the oldest output");
        }
    }
    for input in &edge.order_only_inputs {
        let _ = virtual_mtime(manifest, input, output_map, &mut HashSet::new(), stat_cache)?;
    }
    if !dirty
        && context
            .build_log
            .command_changed(edge, log_command, generator)
    {
        dirty = true;
        reason = "command line changed".to_owned();
    }
    if !dirty && let Some(output) = context.build_log.recorded_mtime_dirty(edge, newest_input) {
        dirty = true;
        reason = format!("recorded mtime of '{output}' is older than an input");
    }
    if !dirty && context.discovered.missing[edge_id] && !generator {
        dirty = true;
        reason = "dependency information is missing".to_owned();
    }

    if !dirty {
        return Ok(EvaluatedEdge {
            dirty: false,
            reason,
            command: String::new(),
            log_command: String::new(),
            description: String::new(),
            pool: None,
            rspfile: None,
            rspfile_content: None,
            newest_input,
        });
    }

    let rspfile = evaluate_unescaped_binding(manifest, edge, "rspfile");
    let command = std::mem::take(command_buffer);
    let log_command = expanded_log_command.unwrap_or_else(|| command.clone());
    Ok(EvaluatedEdge {
        dirty,
        reason,
        description: evaluate_binding(manifest, edge, "description"),
        pool: edge_pool(manifest, edge),
        rspfile: (!rspfile.is_empty()).then(|| PathBuf::from(rspfile)),
        rspfile_content: (!rspfile_content.is_empty()).then_some(rspfile_content),
        newest_input,
        command,
        log_command,
    })
}

fn virtual_mtime(
    manifest: &Manifest,
    path: &str,
    output_map: &HashMap<&str, usize>,
    visiting: &mut HashSet<usize>,
    stat_cache: &mut StatCache<'_>,
) -> Result<u128, String> {
    if let Some(mtime) = stat_cache.checked_get(path)? {
        return Ok(mtime);
    }
    let Some(edge_id) = output_map.get(path).copied() else {
        return Err(format!("input '{path}' is missing"));
    };
    let edge = &manifest.edges[edge_id];
    if edge.rule != "phony" {
        return Err(format!("output '{path}' was not created by its command"));
    }
    if !visiting.insert(edge_id) {
        return Err(format!("phony cycle involving '{path}'"));
    }
    if edge.explicit_inputs.is_empty()
        && edge.implicit_inputs.is_empty()
        && edge.order_only_inputs.is_empty()
        && edge.validations.is_empty()
    {
        visiting.remove(&edge_id);
        return Ok(u128::MAX);
    }
    let mut newest = 0;
    for input in edge.explicit_inputs.iter().chain(&edge.implicit_inputs) {
        newest = newest.max(virtual_mtime(
            manifest, input, output_map, visiting, stat_cache,
        )?);
    }
    visiting.remove(&edge_id);
    Ok(newest)
}

fn evaluate_binding(manifest: &Manifest, edge: &Edge, name: &str) -> String {
    let mut result = String::new();
    evaluate_binding_into(manifest, edge, name, true, &mut result);
    result
}

fn evaluate_unescaped_binding(manifest: &Manifest, edge: &Edge, name: &str) -> String {
    let mut result = String::new();
    evaluate_binding_into(manifest, edge, name, false, &mut result);
    result
}

fn evaluate_binding_into(
    manifest: &Manifest,
    edge: &Edge,
    name: &str,
    escape_paths: bool,
    output: &mut String,
) {
    fn eval(
        manifest: &Manifest,
        edge: &Edge,
        name: &str,
        depth: usize,
        escape_paths: bool,
        output: &mut String,
    ) {
        if depth > 64 {
            return;
        }
        match name {
            "in" => {
                append_path_list(output, &edge.explicit_inputs, ' ', escape_paths);
                return;
            }
            "in_newline" => {
                append_path_list(output, &edge.explicit_inputs, '\n', escape_paths);
                return;
            }
            "out" => {
                append_path_list(output, &edge.explicit_outputs, ' ', escape_paths);
                return;
            }
            _ => {}
        }
        if let Some(value) = edge.bindings.get(name) {
            output.push_str(value);
            return;
        }
        if let Some(raw) = manifest
            .lookup_rule(edge.scope, &edge.rule)
            .and_then(|rule| rule.bindings.get(name))
        {
            expand_eval(raw, manifest, edge, depth, escape_paths, output);
            return;
        }
        if let Some(value) = manifest.lookup_variable(edge.scope, name) {
            output.push_str(value);
        }
    }

    fn expand_eval(
        mut input: &str,
        manifest: &Manifest,
        edge: &Edge,
        depth: usize,
        escape_paths: bool,
        output: &mut String,
    ) {
        while let Some(position) = input.find('$') {
            output.push_str(&input[..position]);
            input = &input[position + 1..];
            let Some(first) = input.chars().next() else {
                output.push('$');
                break;
            };
            match first {
                '$' => {
                    output.push('$');
                    input = &input[1..];
                }
                ' ' => {
                    output.push(' ');
                    input = &input[1..];
                }
                ':' => {
                    output.push(':');
                    input = &input[1..];
                }
                '^' => {
                    output.push('\n');
                    input = &input[1..];
                }
                '{' => {
                    if let Some(end) = input.find('}') {
                        eval(
                            manifest,
                            edge,
                            &input[1..end],
                            depth + 1,
                            escape_paths,
                            output,
                        );
                        input = &input[end + 1..];
                    } else {
                        output.push_str("${");
                        input = &input[1..];
                    }
                }
                _ => {
                    let end = input
                        .find(|character: char| {
                            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
                        })
                        .unwrap_or(input.len());
                    if end == 0 {
                        output.push('$');
                        output.push(first);
                        input = &input[first.len_utf8()..];
                    } else {
                        eval(
                            manifest,
                            edge,
                            &input[..end],
                            depth + 1,
                            escape_paths,
                            output,
                        );
                        input = &input[end..];
                    }
                }
            }
        }
        output.push_str(input);
    }

    eval(manifest, edge, name, 0, escape_paths, output);
}

pub fn render_binding(manifest: &Manifest, edge: &Edge, name: &str) -> String {
    evaluate_binding(manifest, edge, name)
}

pub fn render_unescaped_binding(manifest: &Manifest, edge: &Edge, name: &str) -> String {
    evaluate_unescaped_binding(manifest, edge, name)
}

fn append_path_list(output: &mut String, paths: &[String], separator: char, escape: bool) {
    for (index, path) in paths.iter().enumerate() {
        if index != 0 {
            output.push(separator);
        }
        if escape
            && path
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte == b'"')
        {
            output.push_str(&shell_escape_path(path));
        } else {
            output.push_str(path);
        }
    }
}

#[cfg(windows)]
pub fn shell_escape_path(path: &str) -> String {
    if !path
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'"')
    {
        return path.to_owned();
    }
    let mut escaped = String::with_capacity(path.len() + 2);
    escaped.push('"');
    let mut backslashes = 0usize;
    for character in path.chars() {
        if character == '\\' {
            backslashes += 1;
        } else if character == '"' {
            escaped.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            escaped.push('"');
            backslashes = 0;
        } else {
            escaped.extend(std::iter::repeat_n('\\', backslashes));
            backslashes = 0;
            escaped.push(character);
        }
    }
    escaped.extend(std::iter::repeat_n('\\', backslashes * 2));
    escaped.push('"');
    escaped
}

#[cfg(not(windows))]
pub fn shell_escape_path(path: &str) -> String {
    if path
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"_+-./".contains(&c))
    {
        path.to_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

fn edge_pool(manifest: &Manifest, edge: &Edge) -> Option<String> {
    let pool = evaluate_binding(manifest, edge, "pool");
    (!pool.is_empty()).then_some(pool)
}

fn pool_limit(manifest: &Manifest, pool: Option<&str>, jobs: usize) -> usize {
    match pool {
        Some("console") => 1,
        Some(pool) => manifest.pools.get(pool).map_or(jobs, |pool| {
            if pool.depth == 0 {
                usize::MAX
            } else {
                pool.depth
            }
        }),
        None => jobs,
    }
}

fn limited_pool(manifest: &Manifest, edge: &Edge) -> Option<(String, usize)> {
    let pool = edge_pool(manifest, edge)?;
    let limit = pool_limit(manifest, Some(&pool), usize::MAX);
    (limit != usize::MAX).then_some((pool, limit))
}

fn admit_pool_edges(
    manifest: &Manifest,
    ready: &mut BinaryHeap<(usize, Reverse<usize>)>,
    waiting: &mut HashMap<String, BinaryHeap<(usize, Reverse<usize>)>>,
    reserved: &mut [bool],
    usage: &mut HashMap<String, usize>,
    initially_dirty: &[bool],
) {
    let mut admitted = Vec::with_capacity(ready.len());
    while let Some(candidate @ (_, Reverse(edge_id))) = ready.pop() {
        if reserved[edge_id] || !initially_dirty[edge_id] {
            admitted.push(candidate);
            continue;
        }
        let Some((pool, _)) = limited_pool(manifest, &manifest.edges[edge_id]) else {
            admitted.push(candidate);
            continue;
        };
        waiting.entry(pool).or_default().push(candidate);
    }

    for (pool, edges) in waiting.iter_mut() {
        let limit = pool_limit(manifest, Some(pool), usize::MAX);
        let count = usage.entry(pool.clone()).or_default();
        while *count < limit {
            let Some(candidate @ (_, Reverse(edge_id))) = edges.pop() else {
                break;
            };
            reserved[edge_id] = true;
            *count += 1;
            admitted.push(candidate);
        }
    }
    ready.extend(admitted);
}

fn reserve_new_pool_edges(
    manifest: &Manifest,
    newly_ready: &mut Vec<usize>,
    reserved: &mut [bool],
    usage: &mut HashMap<String, usize>,
    initially_dirty: &[bool],
) {
    for edge_id in newly_ready.drain(..) {
        if reserved[edge_id] || !initially_dirty[edge_id] {
            continue;
        }
        let Some((pool, limit)) = limited_pool(manifest, &manifest.edges[edge_id]) else {
            continue;
        };
        let count = usage.entry(pool).or_default();
        if *count < limit {
            reserved[edge_id] = true;
            *count += 1;
        }
    }
}

fn release_pool_edge(
    manifest: &Manifest,
    edge_id: usize,
    reserved: &mut [bool],
    usage: &mut HashMap<String, usize>,
) {
    if !reserved[edge_id] {
        return;
    }
    reserved[edge_id] = false;
    if let Some((pool, _)) = limited_pool(manifest, &manifest.edges[edge_id])
        && let Some(count) = usage.get_mut(&pool)
    {
        *count = count.saturating_sub(1);
    }
}

fn critical_path_weights(
    manifest: &Manifest,
    closure: &[usize],
    outputs: &HashMap<&str, usize>,
    discovered: &DiscoveredDeps,
) -> Vec<usize> {
    fn edge_weight(edge: &Edge) -> usize {
        usize::from(edge.rule != "phony")
    }

    let mut weights = vec![0; manifest.edges.len()];
    for &edge_id in closure {
        weights[edge_id] = edge_weight(&manifest.edges[edge_id]);
    }
    for &edge_id in closure.iter().rev() {
        let edge = &manifest.edges[edge_id];
        let downstream = weights[edge_id];
        for input in edge
            .inputs()
            .chain(discovered.inputs[edge_id].iter().map(String::as_str))
        {
            let Some(&producer) = outputs.get(input) else {
                continue;
            };
            weights[producer] = weights[producer]
                .max(downstream.saturating_add(edge_weight(&manifest.edges[producer])));
        }
    }
    weights
}

fn can_run_more(running: usize, options: &BuildOptions) -> bool {
    if options.jobserver.is_none() && running >= options.jobs.max(1) {
        return false;
    }
    if options.max_load_average <= 0.0 {
        return true;
    }
    options.max_load_average - system_load_average() >= 1.0 || running == 0
}

#[derive(Debug)]
enum JobSlot {
    Implicit,
    Explicit { _token: jobserver::Acquired },
}

#[cfg(windows)]
fn system_load_average() -> f64 {
    use std::sync::{Mutex, OnceLock};
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    #[derive(Default)]
    struct LoadState {
        idle: u64,
        total: u64,
        load: f64,
    }
    static STATE: OnceLock<Mutex<LoadState>> = OnceLock::new();
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all three pointers reference writable FILETIME values.
    if unsafe { GetSystemTimes(&raw mut idle, &raw mut kernel, &raw mut user) } == 0 {
        return -0.0;
    }
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    let idle = ticks(idle);
    let total = ticks(kernel) + ticks(user);
    let mut state = STATE
        .get_or_init(|| Mutex::new(LoadState::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.total != 0 && total != state.total {
        let idle_delta = idle.saturating_sub(state.idle);
        let total_delta = total.saturating_sub(state.total);
        let instantaneous = 1.0 - idle_delta as f64 / total_delta as f64;
        state.load = if state.load > 0.0 {
            0.9 * state.load + 0.1 * instantaneous
        } else {
            instantaneous
        };
    }
    state.idle = idle;
    state.total = total;
    state.load * thread::available_parallelism().map_or(1, usize::from) as f64
}

#[cfg(unix)]
fn system_load_average() -> f64 {
    let mut loads = [0.0f64; 1];
    // SAFETY: the array contains writable space for the one requested value.
    if unsafe { libc::getloadavg(loads.as_mut_ptr(), 1) } == 1 {
        loads[0]
    } else {
        -0.0
    }
}

#[cfg(not(any(windows, unix)))]
fn system_load_average() -> f64 {
    -0.0
}

fn truthy(value: &str) -> bool {
    !value.is_empty() && value != "0"
}

#[cfg(not(windows))]
fn modified_ns(path: &Path) -> Option<u128> {
    metadata_modified(&path.metadata().ok()?)
}

#[cfg(not(windows))]
fn checked_modified_ns(path: &Path) -> Result<Option<u128>, String> {
    match path.metadata() {
        Ok(metadata) => Ok(metadata_modified(&metadata)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => {
            let message = error.to_string();
            let message = message
                .split_once(" (os error ")
                .map_or(message.as_str(), |(message, _)| message);
            Err(format!("stat({}): {message}", path.display()))
        }
    }
}

#[cfg(not(windows))]
fn metadata_modified(metadata: &fs::Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos().max(1))
}

#[cfg(windows)]
fn modified_ns(path: &Path) -> Option<u128> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileAttributesExW, GetFileExInfoStandard, WIN32_FILE_ATTRIBUTE_DATA,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut data = std::mem::MaybeUninit::<WIN32_FILE_ATTRIBUTE_DATA>::uninit();
    // SAFETY: `wide` is NUL-terminated and `data` points to writable storage of
    // the exact structure requested by GetFileAttributesExW.
    let ok = unsafe {
        GetFileAttributesExW(
            wide.as_ptr(),
            GetFileExInfoStandard,
            data.as_mut_ptr().cast(),
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: a successful call initialized the complete structure.
    let data = unsafe { data.assume_init() };
    let filetime = ((data.ftLastWriteTime.dwHighDateTime as u64) << 32)
        | data.ftLastWriteTime.dwLowDateTime as u64;
    Some(filetime.saturating_sub(126_227_704_000_000_000) as u128)
}

#[cfg(windows)]
fn checked_modified_ns(path: &Path) -> Result<Option<u128>, String> {
    Ok(modified_ns(path))
}

fn finish_command(
    mut command: Command,
    console: bool,
    jobserver: Option<&jobserver::Client>,
) -> io::Result<Output> {
    ensure_process_tree_cleanup().map_err(io::Error::other)?;
    if let Some(jobserver) = jobserver {
        jobserver.configure_make(&mut command);
    }
    if console {
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        configure_process_group(&mut command);
        let mut child = command.spawn()?;
        register_process_group(child.id());
        let status = child.wait();
        unregister_process_group(child.id());
        let status = status?;
        Ok(Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    } else {
        let (mut reader, writer) = os_pipe::pipe()?;
        let stderr = writer.try_clone()?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(writer))
            .stderr(Stdio::from(stderr));
        configure_process_group(&mut command);
        let mut child = command.spawn()?;
        register_process_group(child.id());
        drop(command);
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        let status = child.wait();
        unregister_process_group(child.id());
        let status = status?;
        Ok(Output {
            status,
            stdout: output,
            stderr: Vec::new(),
        })
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn register_process_group(id: u32) {
    ACTIVE_PROCESS_GROUPS
        .get_or_init(|| std::sync::Mutex::new(std::collections::BTreeSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id);
}

#[cfg(not(unix))]
fn register_process_group(_id: u32) {}

#[cfg(unix)]
fn unregister_process_group(id: u32) {
    if let Some(groups) = ACTIVE_PROCESS_GROUPS.get() {
        groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
    }
}

#[cfg(not(unix))]
fn unregister_process_group(_id: u32) {}

struct BuildOutput {
    smart_terminal: bool,
    supports_color: bool,
    have_blank_line: bool,
    buffered: bool,
    suspended: bool,
    pending: Vec<u8>,
}

#[cfg(windows)]
const BUILD_NEWLINE: &[u8] = b"\r\n";
#[cfg(not(windows))]
const BUILD_NEWLINE: &[u8] = b"\n";

fn write_build_text(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    #[cfg(not(windows))]
    {
        writer.write_all(bytes)
    }
    #[cfg(windows)]
    {
        let inserted = bytes
            .iter()
            .enumerate()
            .filter(|(index, byte)| **byte == b'\n' && (*index == 0 || bytes[*index - 1] != b'\r'))
            .count();
        if inserted == 0 {
            return writer.write_all(bytes);
        }
        let mut translated = Vec::with_capacity(bytes.len() + inserted);
        let mut start = 0;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' && (index == 0 || bytes[index - 1] != b'\r') {
                translated.extend_from_slice(&bytes[start..index]);
                translated.extend_from_slice(BUILD_NEWLINE);
                start = index + 1;
            }
        }
        translated.extend_from_slice(&bytes[start..]);
        writer.write_all(&translated)
    }
}

impl BuildOutput {
    fn new(fancy_status: bool, buffer_redirected_output: bool) -> Self {
        let is_terminal = io::stdout().is_terminal();
        let term = std::env::var_os("TERM");
        let smart_terminal = fancy_status
            && smart_terminal_policy(
                is_terminal,
                term.is_some(),
                term.is_some_and(|value| value == "dumb"),
                cfg!(windows),
            );
        Self {
            smart_terminal,
            supports_color: command_output_supports_color(is_terminal),
            have_blank_line: true,
            buffered: buffer_redirected_output && !smart_terminal,
            suspended: false,
            pending: Vec::new(),
        }
    }

    fn is_smart_terminal(&self) -> bool {
        self.smart_terminal
    }

    fn supports_color(&self) -> bool {
        self.supports_color
    }

    fn suspend(&mut self) {
        self.suspended = true;
    }

    fn resume(&mut self) -> Result<(), String> {
        self.suspended = false;
        if self.buffered || self.pending.is_empty() {
            return Ok(());
        }
        let mut stdout = io::stdout().lock();
        write_build_text(&mut stdout, &self.pending)
            .and_then(|()| stdout.flush())
            .map_err(|error| format!("writing buffered console output: {error}"))?;
        self.pending.clear();
        Ok(())
    }

    fn print_status(&mut self, line: &str, full: bool) -> Result<(), String> {
        if self.buffered || self.suspended {
            self.pending.extend_from_slice(line.as_bytes());
            self.pending.extend_from_slice(BUILD_NEWLINE);
            return Ok(());
        }
        let mut stdout = io::stdout().lock();
        if self.smart_terminal {
            stdout
                .write_all(b"\r")
                .map_err(|error| format!("writing build status: {error}"))?;
        }
        write_build_text(&mut stdout, line.as_bytes())
            .map_err(|error| format!("writing build status: {error}"))?;
        if self.smart_terminal && !full {
            stdout
                .write_all(b"\x1b[K")
                .map_err(|error| format!("writing build status: {error}"))?;
            self.have_blank_line = false;
        } else {
            stdout
                .write_all(BUILD_NEWLINE)
                .map_err(|error| format!("writing build status: {error}"))?;
        }
        stdout
            .flush()
            .map_err(|error| format!("writing build status: {error}"))
    }

    fn print_on_new_line(&mut self, output: &[u8]) -> Result<(), String> {
        if self.buffered || self.suspended {
            if !self.have_blank_line {
                self.pending.extend_from_slice(BUILD_NEWLINE);
            }
            self.pending.extend_from_slice(output);
            self.have_blank_line = output.is_empty() || output.ends_with(b"\n");
            return Ok(());
        }
        let mut stdout = io::stdout().lock();
        if !self.have_blank_line {
            stdout
                .write_all(BUILD_NEWLINE)
                .map_err(|error| format!("writing command output: {error}"))?;
        }
        write_build_text(&mut stdout, output)
            .map_err(|error| format!("writing command output: {error}"))?;
        self.have_blank_line = output.is_empty() || output.ends_with(b"\n");
        Ok(())
    }

    fn write_command_output(&mut self, output: &[u8]) -> Result<(), String> {
        if output.is_empty() {
            return Ok(());
        }
        if self.supports_color {
            self.print_on_new_line(output)
        } else {
            self.print_on_new_line(&strip_ansi_escapes::strip(output))
        }
    }

    fn finish_line(&mut self) -> Result<(), String> {
        self.print_on_new_line(&[])?;
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut stdout = io::stdout().lock();
        write_build_text(&mut stdout, &self.pending)
            .and_then(|()| stdout.flush())
            .map_err(|error| format!("writing build output: {error}"))?;
        self.pending.clear();
        Ok(())
    }
}

fn smart_terminal_policy(
    is_terminal: bool,
    term_is_set: bool,
    term_is_dumb: bool,
    windows: bool,
) -> bool {
    is_terminal && !term_is_dumb && (windows || term_is_set)
}

fn command_output_supports_color(is_terminal: bool) -> bool {
    fn enabled(name: &str) -> bool {
        std::env::var_os(name).is_some_and(|value| value != "0")
    }

    color_policy(
        is_terminal,
        std::env::var_os("TERM").is_some_and(|value| value == "dumb"),
        enabled("NO_COLOR"),
        enabled("CLICOLOR_FORCE"),
        enabled("FORCE_COLOR"),
    )
}

fn color_policy(
    is_terminal: bool,
    term_is_dumb: bool,
    no_color: bool,
    clicolor_force: bool,
    force_color: bool,
) -> bool {
    if force_color {
        return true;
    }
    if no_color {
        return false;
    }
    (is_terminal && !term_is_dumb) || clicolor_force
}

#[cfg(windows)]
fn execute_command(
    command: &str,
    console: bool,
    jobserver: Option<&jobserver::Client>,
) -> io::Result<Output> {
    use std::os::windows::process::CommandExt;

    let command = command.trim_start();
    let (program, arguments) = if let Some(quoted) = command.strip_prefix('"') {
        let end = quoted.find('"').ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "unterminated executable quote")
        })?;
        (&quoted[..end], quoted[end + 1..].trim_start())
    } else {
        let end = command.find(char::is_whitespace).unwrap_or(command.len());
        (&command[..end], command[end..].trim_start())
    };
    if program.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty command"));
    }
    let mut child = Command::new(program);
    if !arguments.is_empty() {
        child.raw_arg(arguments);
    }
    finish_command(child, console, jobserver)
}

#[cfg(not(windows))]
fn execute_command(
    command: &str,
    console: bool,
    jobserver: Option<&jobserver::Client>,
) -> io::Result<Output> {
    let mut child = Command::new("sh");
    child.args(["-c", command]);
    finish_command(child, console, jobserver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_manifest;
    use tempfile::tempdir;

    fn build_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn default_parallelism_matches_ninja() {
        assert_eq!(guess_parallelism(0), 2);
        assert_eq!(guess_parallelism(1), 2);
        assert_eq!(guess_parallelism(2), 3);
        assert_eq!(guess_parallelism(8), 10);
    }

    #[test]
    fn color_policy_matches_ninja_environment_precedence() {
        assert!(color_policy(true, false, false, false, false));
        assert!(!color_policy(true, true, false, false, false));
        assert!(!color_policy(true, false, true, true, false));
        assert!(color_policy(false, false, false, true, false));
        assert!(color_policy(false, true, true, false, true));
    }

    #[test]
    fn smart_terminal_policy_matches_ninja_platform_split() {
        assert!(!smart_terminal_policy(false, true, false, false));
        assert!(!smart_terminal_policy(true, false, false, false));
        assert!(smart_terminal_policy(true, true, false, false));
        assert!(!smart_terminal_policy(true, true, true, false));
        assert!(smart_terminal_policy(true, false, false, true));
        assert!(!smart_terminal_policy(true, true, true, true));
    }

    #[test]
    fn zero_depth_pool_is_unlimited() {
        let manifest = parse_manifest("pool p\n  depth = 0\n", "build.ninja").unwrap();
        assert_eq!(pool_limit(&manifest, Some("p"), 12), usize::MAX);
    }

    #[test]
    fn missing_or_non_directory_parents_authoritatively_have_no_entries() {
        let temp = tempdir().unwrap();
        assert!(matches!(
            directory_mtimes(&temp.path().join("missing")),
            DirectoryMtimes::Missing
        ));
        let file = temp.path().join("file");
        fs::write(&file, "not a directory").unwrap();
        assert!(matches!(directory_mtimes(&file), DirectoryMtimes::Missing));
    }

    #[test]
    fn expands_all_ninja_status_placeholders() {
        let mut status = StatusFormatter::new(2);
        let rendered = status
            .format(
                "[%% %s/%t %f %r %u %p %o %c %e %w %E %W %P] ",
                false,
                StatusSnapshot {
                    started: 2,
                    finished: 1,
                    running: 1,
                    total: 4,
                    description: "compile",
                    elapsed: Duration::from_millis(2_000),
                },
            )
            .unwrap();
        assert_eq!(
            rendered,
            "[% 2/4 1 1 2  25% 0.5 ? 2.000 00:02 6.000 00:06  25%] compile"
        );
    }

    #[test]
    fn status_prediction_uses_previous_cpu_times() {
        let mut prediction = ProgressPrediction::default();
        prediction.add_edge(Some(1_000));
        prediction.add_edge(Some(9_000));
        prediction.finish_edge(Some(1_000), Duration::from_millis(1_000));

        assert!((prediction.percentage(Duration::from_secs(1)) - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn status_prediction_rejects_wildly_stale_history() {
        let mut prediction = ProgressPrediction::default();
        for _ in 0..20 {
            prediction.add_edge(Some(100));
        }
        prediction.finish_edge(Some(100), Duration::from_secs(16));

        assert!((prediction.percentage(Duration::from_secs(16)) - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn status_prediction_history_is_loaded_only_when_referenced() {
        assert!(!status_needs_prediction("[%f/%t] ", false));
        assert!(!status_needs_prediction("%%P %p", false));
        assert!(status_needs_prediction("%P %E %W", false));
        assert!(!status_needs_prediction("$progress", true));
        assert!(status_needs_prediction("$predicted_progress", true));
        assert!(status_needs_prediction("$eta_seconds", true));
    }

    #[test]
    fn expands_status_variables_without_implicit_description() {
        let mut status = StatusFormatter::new(2);
        let rendered = status
            .format(
                "$finished/$total $$ $: $ description=$description",
                true,
                StatusSnapshot {
                    started: 1,
                    finished: 1,
                    running: 0,
                    total: 1,
                    description: "compile",
                    elapsed: Duration::from_secs(1),
                },
            )
            .unwrap();
        assert_eq!(rendered, "1/1 $ :  description=compile");

        let rendered = status
            .format(
                "$finished",
                true,
                StatusSnapshot {
                    started: 1,
                    finished: 1,
                    running: 0,
                    total: 1,
                    description: "compile",
                    elapsed: Duration::from_secs(1),
                },
            )
            .unwrap();
        assert_eq!(rendered, "1");
    }

    #[test]
    fn rejects_unknown_status_tokens() {
        let mut status = StatusFormatter::new(1);
        assert!(
            status
                .format(
                    "%x",
                    false,
                    StatusSnapshot {
                        started: 1,
                        finished: 1,
                        running: 0,
                        total: 1,
                        description: "",
                        elapsed: Duration::ZERO,
                    },
                )
                .unwrap_err()
                .contains("unknown placeholder")
        );
        assert!(
            status
                .format(
                    "$mystery",
                    true,
                    StatusSnapshot {
                        started: 1,
                        finished: 1,
                        running: 0,
                        total: 1,
                        description: "",
                        elapsed: Duration::ZERO,
                    },
                )
                .unwrap_err()
                .contains("unknown variable")
        );
    }

    #[test]
    fn eagerly_expanded_bindings_remain_literal_in_rule_expansion() {
        let manifest = parse_manifest(
            "bar = X\nfoo = $$bar\nrule echo\n  command = echo $foo\nbuild out: echo\n",
            "build.ninja",
        )
        .unwrap();
        assert_eq!(
            render_binding(&manifest, &manifest.edges[0], "command"),
            "echo $bar"
        );

        let manifest = parse_manifest(
            "name = expanded\nliteral = $$name\nrule echo\n  command = echo $value\nbuild out: echo\n  value = $literal\n",
            "build.ninja",
        )
        .unwrap();
        assert_eq!(
            render_binding(&manifest, &manifest.edges[0], "command"),
            "echo $name"
        );
    }

    #[test]
    fn path_bindings_can_disable_in_out_shell_escaping() {
        let manifest = parse_manifest(
            concat!(
                "rule cc\n",
                "  command = cc @$rspfile $in\n",
                "  depfile = $out.d\n",
                "  rspfile = $out.rsp\n",
                "  rspfile_content = $in_newline\n",
                "build foo$ bar.o: cc source$ file.c\n",
            ),
            "build.ninja",
        )
        .unwrap();
        let edge = &manifest.edges[0];
        assert_eq!(
            evaluate_unescaped_binding(&manifest, edge, "depfile"),
            "foo bar.o.d"
        );
        assert_eq!(
            evaluate_unescaped_binding(&manifest, edge, "rspfile"),
            "foo bar.o.rsp"
        );
        assert!(render_binding(&manifest, edge, "command").contains("source file.c"));
    }

    #[test]
    fn builds_incrementally() {
        let _lock = build_test_lock();
        let temp = tempdir().unwrap();
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();
        #[cfg(windows)]
        let command = "cmd /c echo hello>$out";
        #[cfg(not(windows))]
        let command = "printf hello > $out";
        let manifest = parse_manifest(
            &format!("rule write\n  command = {command}\nbuild out.txt: write\ndefault out.txt\n"),
            "build.ninja",
        )
        .unwrap();
        let first = run_build(&manifest, &[], &BuildOptions::default()).unwrap();
        let second = run_build(&manifest, &[], &BuildOptions::default()).unwrap();
        std::env::set_current_dir(old).unwrap();
        assert_eq!(first.commands_run, 1);
        assert_eq!(second.commands_run, 0);
    }

    #[test]
    fn accepts_older_outputs_and_first_output_records_from_deps_log() {
        let _lock = build_test_lock();
        let temp = tempdir().unwrap();
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();
        fs::write("out1", "one").unwrap();
        fs::write("out2", "two").unwrap();
        let output_mtime = modified_ns(Path::new("out1")).unwrap() as u64;
        let mut log = DepsLog::load(PathBuf::from(".ninja_deps")).unwrap();
        log.record(
            "out1",
            output_mtime.saturating_add(1),
            &["header.h".to_owned()],
        )
        .unwrap();

        let manifest = parse_manifest(
            concat!(
                "rule cc\n",
                "  command = cc\n",
                "  deps = gcc\n",
                "  depfile = deps.d\n",
                "build out1 out2: cc source.c\n",
            ),
            "build.ninja",
        )
        .unwrap();
        let discovered = DiscoveredDeps::load(&manifest);
        std::env::set_current_dir(old).unwrap();

        assert_eq!(discovered.inputs[0], ["header.h"]);
        assert!(!discovered.missing[0]);
    }

    #[test]
    fn dyndep_prebuild_includes_safe_work_but_holds_consumers() {
        let manifest = parse_manifest(
            concat!(
                "rule run\n  command = run\n",
                "build independent: run\n",
                "build dd: run\n",
                "build consumer: run independent || dd\n  dyndep = dd\n",
                "default independent consumer\n",
            ),
            "build.ninja",
        )
        .unwrap();
        let outputs = output_map(&manifest);
        let discovered = DiscoveredDeps::load(&manifest);
        let targets = select_targets(&manifest, &[], &outputs, &discovered.log).unwrap();
        let closure =
            dependency_closure(&manifest, &targets, &outputs, &discovered, false).unwrap();
        let files = pending_dyndep_files(&manifest, &closure, &HashSet::new()).unwrap();
        assert_eq!(
            dyndep_prebuild_targets(&manifest, &closure, &outputs, &discovered, &files),
            ["dd", "independent"]
        );
    }

    #[test]
    fn detects_cycles() {
        let _lock = build_test_lock();
        let manifest = parse_manifest(
            "build a: phony b\nbuild b: phony a\ndefault a\n",
            "build.ninja",
        )
        .unwrap();
        let error = run_build(&manifest, &[], &BuildOptions::default()).unwrap_err();
        assert!(error.contains("cycle"));
    }

    #[test]
    fn resolves_deep_dependency_chains_without_using_the_call_stack() {
        let count = 50_000;
        let mut manifest = Manifest::default();
        manifest.edges.reserve(count);
        for index in 0..count {
            let mut edge = Edge {
                explicit_outputs: vec![format!("out/{index}")],
                ..Edge::default()
            };
            if index != 0 {
                edge.explicit_inputs.push(format!("out/{}", index - 1));
            }
            manifest.edges.push(edge);
        }
        let outputs = output_map(&manifest);
        let discovered = DiscoveredDeps {
            inputs: vec![Vec::new(); count],
            missing: vec![false; count],
            errors: vec![None; count],
            log: DepsLog::default(),
        };
        let closure =
            dependency_closure(&manifest, &[count - 1], &outputs, &discovered, false).unwrap();
        assert_eq!(closure.len(), count);
        assert_eq!(closure[0], 0);
        assert_eq!(closure[count - 1], count - 1);
    }

    #[test]
    fn command_hash_matches_ninja_v7_log() {
        let command = b"cmd /c echo hello>out.txt";
        assert_eq!(hash(command), 0x29ceb1f38b6e5e65);
    }

    #[test]
    fn build_log_recompacts_redundant_entries_automatically() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_log");
        let mut contents = String::from("# ninja log v7\n");
        for index in 0..101 {
            contents.push_str(&format!("{index}\t{index}\t42\tout\t1\n"));
        }
        fs::write(&path, contents).unwrap();
        let manifest = parse_manifest("build out: phony\n", "build.ninja").unwrap();
        let outputs = output_map(&manifest);
        let log = BuildLog::load(path.clone(), &outputs).unwrap();
        assert_eq!(log.entries["out"].mtime, 42);
        let compacted = fs::read_to_string(path).unwrap();
        assert_eq!(compacted.lines().count(), 2);
        assert!(compacted.contains("100\t100\t42\tout\t1"));
    }

    #[test]
    fn incompatible_build_log_is_discarded() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_log");
        fs::write(&path, "# ninja log v6\n0\t0\t0\tout\t1\n").unwrap();
        let manifest = parse_manifest("build out: phony\n", "build.ninja").unwrap();
        let outputs = output_map(&manifest);
        assert!(
            BuildLog::load(path.clone(), &outputs)
                .unwrap()
                .entries
                .is_empty()
        );
        assert!(!path.exists());
    }

    #[test]
    fn truncated_build_log_tail_is_removed_before_append() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_log");
        fs::write(&path, "# ninja log v7\n0\t1\t2\tout\t1\n3\t4\t5\ttruncated").unwrap();
        let manifest = parse_manifest("build out: phony\n", "build.ninja").unwrap();
        let outputs = output_map(&manifest);
        let log = BuildLog::load(path.clone(), &outputs).unwrap();
        assert_eq!(log.entries["out"].mtime, 2);
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "# ninja log v7\n0\t1\t2\tout\t1\n"
        );
    }

    #[test]
    fn build_log_recompaction_keeps_existing_or_live_outputs() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".ninja_log");
        let existing = temp.path().join("generator-output");
        fs::write(&existing, b"").unwrap();
        let missing = temp.path().join("deleted-output");
        let contents = format!(
            "# ninja log v7\n0\t0\t1\tlive\t1\n0\t0\t1\t{}\t1\n0\t0\t1\t{}\t1\n",
            existing.display(),
            missing.display()
        );
        let live = [("live", 0usize)].into_iter().collect::<HashMap<_, _>>();
        recompact_build_log(&path, &contents, &live).unwrap();
        let compacted = fs::read_to_string(path).unwrap();
        assert!(compacted.contains("\tlive\t"));
        assert!(compacted.contains(&format!("\t{}\t", existing.display())));
        assert!(!compacted.contains(&format!("\t{}\t", missing.display())));
    }

    #[test]
    fn parses_gcc_depfiles_with_continuations_and_escaped_spaces() {
        let deps = parse_depfile(concat!(
            "out.o: source.c include/a.h \\\n",
            "  include/with\\ space.h\n"
        ))
        .unwrap();
        assert_eq!(
            deps.inputs,
            ["source.c", "include/a.h", "include/with space.h"]
        );
        assert_eq!(
            parse_depfile("out.o: C:\\src\\answer.c C:\\inc\\answer.h\n")
                .unwrap()
                .inputs,
            ["C:\\src\\answer.c", "C:\\inc\\answer.h"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn escapes_windows_command_line_paths() {
        assert_eq!(shell_escape_path("plain/path"), "plain/path");
        assert_eq!(
            shell_escape_path("path with space\\"),
            "\"path with space\\\\\""
        );
        assert_eq!(
            shell_escape_path("a \\\"quoted\" b"),
            "\"a \\\\\\\"quoted\\\" b\""
        );
    }

    #[test]
    fn filters_msvc_includes_and_compiler_input_echo() {
        let mut includes = BTreeSet::new();
        let filtered = filter_msvc_output(
            b"source.c\r\nNote: including file:   include\\local.h\r\nwarning: hello\r\n",
            "Note: including file: ",
            &mut includes,
        )
        .unwrap();
        assert_eq!(String::from_utf8(filtered).unwrap(), "warning: hello\n");
        assert_eq!(includes.len(), 1);
        assert!(includes.iter().next().unwrap().ends_with("include/local.h"));
    }

    #[test]
    fn collects_transitive_validations_without_creating_false_cycles() {
        let manifest = parse_manifest(
            concat!(
                "build base: phony\n",
                "build check: phony base\n",
                "build middle: phony base |@ check\n",
                "build goal: phony middle\n",
                "default goal\n",
            ),
            "build.ninja",
        )
        .unwrap();
        let outputs = output_map(&manifest);
        let discovered = DiscoveredDeps::load(&manifest);
        let closure =
            dependency_closure(&manifest, &[outputs["goal"]], &outputs, &discovered, false)
                .unwrap();
        assert!(closure.contains(&outputs["check"]));
        assert_eq!(closure.len(), 4);
    }

    #[test]
    fn phony_self_cycle_warning_mode_ignores_the_self_input() {
        let _lock = build_test_lock();
        let manifest =
            parse_manifest("build all: phony all\ndefault all\n", "build.ninja").unwrap();
        run_build(&manifest, &[], &BuildOptions::default()).unwrap();
        let strict = BuildOptions {
            phony_cycle_error: true,
            ..BuildOptions::default()
        };
        assert!(
            run_build(&manifest, &[], &strict)
                .unwrap_err()
                .contains("cycle")
        );
    }
}
