#![deny(warnings)]
#![cfg_attr(feature = "cargo-clippy", deny(clippy::all))]

extern crate notify;
extern crate ignore;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use notify::Watcher;
use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};

const USAGE: &str = "auto-check-rs

Usage:
    auto-check-rs [options] [-vvvv] <crate-dir>
    auto-check-rs (-h | --help)
    auto-check-rs --version

Options:
    -h --help                       Show this screen.
    --version                       Show version.
    -v --verbose                    Increase the verbosity level, default is only errors
    --delay=MS                      Delay in milliseconds before triggering [default: 1000]
    -c --custom-cmd=CMD             Run the specified command without arguments after the other checks
    --no-run-first                  Don't always run once after startup, wait for a change
    --no-check                      Don't run cargo check
    --no-clippy                     Don't run cargo clippy
    --no-test                       Don't run cargo test
";

enum Action {
    Nothing,
    Custom(String),
    FilesChanged(Vec<PathBuf>),
}

struct Changes {
    base_dir: PathBuf,
    gitignore: Gitignore,
    ignore_changes: Arc<AtomicBool>,
    custom: Option<String>,
    changed: BTreeSet<PathBuf>,
}

impl Changes {
    fn new<P: Into<PathBuf>>(base_dir: P, gitignore: Gitignore) -> Changes {
        let base_dir = base_dir.into();
        assert!(base_dir.is_absolute());
        Changes {
            base_dir,
            gitignore,
            ignore_changes: Default::default(),
            custom: None,
            changed: Default::default(),
        }
    }

    fn add_custom<T: Into<String>>(&mut self, reason: T) {
        self.custom = Some(reason.into());
    }

    fn add<P: AsRef<Path>>(&mut self, fpath: &P) {
        let ignore = self.ignore_changes.load(Ordering::Relaxed);
        let fpath = fpath.as_ref();
        match fpath.strip_prefix(&self.base_dir) {
            Ok(fpath) => match self.gitignore.matched_path_or_any_parents(fpath, false) {
                Match::Ignore(_) => {
                    log::trace!("Ignoring path from .gitignore: {}", fpath.to_string_lossy());
                },
                Match::Whitelist(_) | Match::None => {
                    if ignore {
                        log::debug!("Ignored change: {}", fpath.to_string_lossy());
                    } else {
                        log::debug!("Detected change: {}", fpath.to_string_lossy());
                        self.changed.insert(fpath.into());
                    }
                },
            },
            Err(_) => {
                log::error!("Ignoring unknown path: {}", fpath.to_string_lossy());
            },
        }
    }

    fn take_current_action(&mut self) -> Action {
        if let Some(reason) = self.custom.take() {
            // Return the custom reason for running
            self.changed = BTreeSet::new(); // Ignore any changes up until now
            self.ignore_changes.store(true, Ordering::Relaxed);
            Action::Custom(reason)
        } else if !self.changed.is_empty() {
            // Return the list of changed files
            let mut changed = BTreeSet::new();
            std::mem::swap(&mut changed, &mut self.changed);
            self.ignore_changes.store(true, Ordering::Relaxed);
            Action::FilesChanged(changed.into_iter().collect())
        } else {
            // There is nothing to do here
            Action::Nothing
        }
    }
}

fn main() {
    //std::env::set_var("RUST_BACKTRACE", "1");

    let args = docopt::Docopt::new(USAGE)
        .and_then(|d| d.parse())
        .unwrap_or_else(|e| e.exit());

    env_logger::builder()
        .filter(None, match args.get_count("--verbose") {
            0 => log::LevelFilter::Error,
            1 => log::LevelFilter::Warn,
            2 => log::LevelFilter::Info,
            3 => log::LevelFilter::Debug,
            _ => log::LevelFilter::Trace,
        })
        .init();

    let mut crate_dir = std::path::PathBuf::from(args.get_str("<crate-dir>"));

    if crate_dir.is_relative() {
        let mut tmp = std::env::current_dir().expect("Failed to get the current directory");
        tmp.push(crate_dir);
        crate_dir = tmp;
        log::debug!("Using crate directory: {}", crate_dir.to_string_lossy());
    }

    let gitignore = {
        let mut builder = GitignoreBuilder::new(&crate_dir);
        // The .git directory is currently not ignored, and
        // there is no way of initializing it like git would yet.
        // See: https://github.com/BurntSushi/ripgrep/issues/1040
        builder
            .add_line(None, "**/.git")
            .expect("Failed to add .git to ignore list");
        builder.add(".gitignore");
        builder.build().expect("Failed to load .gitignore")
    };

    let mut commands_to_run: Vec<Vec<String>> = Vec::new();

    if !args.get_bool("--no-check") {
        commands_to_run.push(vec!["cargo".into(), "check".into()]);
    }

    if !args.get_bool("--no-clippy") {
        commands_to_run.push(vec![
            "cargo".into(),
            "clippy".into(),
            "--all-targets".into(),
            "--all-features".into(),
        ]);
    }

    if !args.get_bool("--no-test") {
        commands_to_run.push(vec!["cargo".into(), "test".into()]);
    }

    let custom_cmd = args.get_str("--custom-cmd");
    if !custom_cmd.is_empty() {
        commands_to_run.push(vec![custom_cmd.into()]);
    }

    if commands_to_run.is_empty() {
        log::error!("Cowardly refusing to start because there is no commands to run");
        std::process::exit(1);
    }

    let delay_ms: u64 = args
        .get_str("--delay")
        .parse()
        .expect("Expected positive number for --delay");
    let delay = std::time::Duration::from_millis(delay_ms);

    let (inotify_tx, inotify_rx) = std::sync::mpsc::channel();
    let (action_tx, action_rx) = std::sync::mpsc::channel::<Action>();

    let mut watcher = notify::watcher(inotify_tx, std::time::Duration::from_millis(100))
        .expect("Failed to initialize inotify watcher");
    watcher
        .watch(&crate_dir, notify::RecursiveMode::Recursive)
        .expect("Failed to add watch");

    let mut changes = Changes::new(&crate_dir, gitignore);
    let ignore_changes = changes.ignore_changes.clone();

    std::thread::spawn(move || {
        for action in action_rx.iter() {
            let run_commands = match action {
                Action::Nothing => {
                    log::trace!("No changes detected");
                    false
                },
                Action::Custom(reason) => {
                    log::info!("{}", reason);
                    true
                },
                Action::FilesChanged(current_paths) => {
                    log::info!("Detected change: {:?}", current_paths);
                    true
                },
            };

            if run_commands {
                'command_loop: for cmd in commands_to_run.iter() {
                    println!();
                    log::info!("Running command {:?}", cmd);
                    let mut command = std::process::Command::new(&cmd[0]);
                    command.current_dir(&crate_dir);
                    command.args(&cmd[1..]);

                    match command.status() {
                        Ok(status) => {
                            if status.success() {
                                log::debug!("Successfully executed {:?}", command);
                            } else {
                                log::error!("Failed to execute {:?}: Returned status {:?}", command, status.code());
                                break 'command_loop;
                            }
                        },
                        Err(e) => {
                            log::error!("Failed to execute {:?}: {:?}", command, e);
                            break 'command_loop;
                        },
                    }
                }
                println!();
                ignore_changes.store(false, Ordering::Relaxed);
            }
        }
    });

    if !args.get_bool("--no-run-first") {
        changes.add_custom("Initial check");
    }

    loop {
        use notify::DebouncedEvent::*;
        use std::sync::mpsc::RecvTimeoutError::*;

        match inotify_rx.recv_timeout(delay) {
            Ok(NoticeWrite(_)) => {},
            Ok(NoticeRemove(_)) => {},
            Ok(Chmod(_)) => {},
            Ok(Create(fpath)) => changes.add(&fpath),
            Ok(Write(fpath)) => changes.add(&fpath),
            Ok(Remove(fpath)) => changes.add(&fpath),
            Ok(Rename(spath, dpath)) => {
                changes.add(&spath);
                changes.add(&dpath);
            },
            Ok(Rescan) => log::warn!("Some issue detected, rescanning all watches"),
            Ok(Error(e, fpath)) => log::error!("{:?} ({:?})", e, fpath),
            Err(Timeout) => {
                action_tx
                    .send(changes.take_current_action())
                    .expect("Failed to publish action");
            },
            Err(e) => panic!("inotify channel died: {:?}", e),
        }
    }
}
