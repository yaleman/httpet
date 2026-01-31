//! HTTPet.org site code

#![allow(clippy::multiple_crate_versions)]
#![deny(clippy::all)]
#![deny(clippy::await_holding_lock)]
#![deny(clippy::complexity)]
#![deny(clippy::correctness)]
#![deny(clippy::disallowed_methods)]
#![deny(clippy::expect_used)]
#![deny(clippy::needless_pass_by_value)]
#![deny(clippy::panic)]
#![deny(clippy::perf)]
#![deny(clippy::trivially_copy_pass_by_ref)]
#![deny(clippy::unreachable)]
#![deny(clippy::unwrap_used)]
#![deny(warnings)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod cli;
pub mod config;
pub mod constants;
pub mod db;
pub mod error;
pub mod web;
