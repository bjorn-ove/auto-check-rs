[![Build Status](https://travis-ci.org/BearOve/auto-check-rs.svg?branch=master)](https://travis-ci.org/BearOve/auto-check-rs)
[![crates.io](https://meritbadge.herokuapp.com/auto-check-rs)](https://crates.io/crates/auto-check-rs)

# auto-check-rs

This is a simple tool designed to run in a split pane next to the editor to automatically build, check and test the code.

## Purpose

I wrote this for myself to use together with tmux and [amp.rs](https://amp.rs). I mainly made it public for my own convinience.

## Known issue with the cargo target directory

When used with a rust crate the reccomended approach is to run it using a target directory outside of the crate. The way I do
it is to run it using `CARGO_TARGET_DIR="$HOME/.cache/rust/my-crate/target" auto-check-rs -vv .`. The reason for this is issues
with inotify when a lot of files are being ignored. This may be fixable in this crate, but I haven't had the time to debug it
and use a target directory outside as a work-around instead.

## License

This project is licensed under either of [Apache License, Version
2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache 2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
