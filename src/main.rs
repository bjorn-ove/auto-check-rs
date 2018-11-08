#![deny(warnings)]
#![cfg_attr(feature = "cargo-clippy", deny(clippy::all))]

extern crate notify;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use notify::Watcher;

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
";

struct Changes {
    base_dir: PathBuf,
    ignore_changes: Arc<AtomicBool>,
    changed: BTreeSet<PathBuf>,
}

impl Changes {
    fn new<P: Into<PathBuf>>(base_dir: P) -> Changes {
        let base_dir = base_dir.into();
        assert!(base_dir.is_absolute());
        Changes {
            base_dir,
            ignore_changes: Default::default(),
            changed: Default::default(),
        }
    }

    fn add<P: AsRef<Path>>(&mut self, fpath: &P) {
        let ignore = self.ignore_changes.load(Ordering::Relaxed);
        let fpath = fpath.as_ref();
        match fpath.strip_prefix(&self.base_dir) {
            Ok(fpath) => {
                if fpath.starts_with("target/") {
                    log::trace!("Ignoring path inside target/: {}", fpath.to_string_lossy());
                } else {
                    if ignore {
                        log::debug!("Ignored change: {}", fpath.to_string_lossy());
                    } else {
                        log::debug!("Detected change: {}", fpath.to_string_lossy());
                        self.changed.insert(fpath.into());
                    }
                }
            },
            Err(_) => {
                log::error!("Ignoring unknown path: {}", fpath.to_string_lossy());
            }
        }
    }

    fn flush(&mut self) -> Vec<PathBuf> {
        let mut changed = BTreeSet::new();
        std::mem::swap(&mut changed, &mut self.changed);
        if !changed.is_empty() {
            self.ignore_changes.store(true, Ordering::Relaxed);
        }
        changed.into_iter().collect()
    }
}

fn main() {
    //std::env::set_var("RUST_BACKTRACE", "1");

    let args = docopt::Docopt::new(USAGE)
        .and_then(|d| d.parse())
        .unwrap_or_else(|e| e.exit());

    let mut crate_dir = std::path::PathBuf::from(args.get_str("<crate-dir>"));

    if crate_dir.is_relative() {
        let mut tmp = std::env::current_dir().expect("Failed to get the current directory");
        tmp.push(crate_dir);
        crate_dir = tmp;
    }

    pretty_env_logger::formatted_builder()
        .unwrap()
        .filter_level(match args.get_count("--verbose") {
            0 => log::LevelFilter::Error,
            1 => log::LevelFilter::Warn,
            2 => log::LevelFilter::Info,
            3 => log::LevelFilter::Debug,
            _ => log::LevelFilter::Trace,
        })
        .init();

    let mut commands_to_run: Vec<Vec<String>> = Vec::new();

    commands_to_run.push(vec!["cargo".into(), "check".into()]);
    commands_to_run.push(vec!["cargo".into(), "clippy".into()]);
    commands_to_run.push(vec!["cargo".into(), "test".into()]);

    let delay_ms: u64 = args.get_str("--delay").parse().expect("Expected positive number for --delay");
    let delay = std::time::Duration::from_millis(delay_ms);

    let (inotify_tx, inotify_rx) = std::sync::mpsc::channel();
    let (change_tx, change_rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();

    let mut watcher = notify::watcher(inotify_tx, std::time::Duration::from_millis(100))
        .expect("Failed to initialize inotify watcher");
    watcher.watch(&crate_dir, notify::RecursiveMode::Recursive).expect("Failed to add watch");

    let mut changes = Changes::new(&crate_dir);
    let ignore_changes = changes.ignore_changes.clone();

    std::thread::spawn(move || {
        for current_paths in change_rx.iter() {
            if !current_paths.is_empty() {
                log::info!("Detected change: {:?}", current_paths);
                'command_loop: for cmd in commands_to_run.iter() {
                    println!("");
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
                println!("");
                ignore_changes.store(false, Ordering::Relaxed);
            }
        }
    });

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
                change_tx.send(changes.flush()).expect("Failed to publish changed files");
            },
            Err(e) => panic!("inotify channel died: {:?}", e),
        }
    }
}
