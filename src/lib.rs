//! Shared library surface for the `mns` CLI and the `mns-gui` desktop shell.
//!
//! The investment engine lives here exactly once. `src/main.rs` is a thin
//! presentation layer over these modules, and `src/app` is the typed
//! application service the desktop UI calls. Neither front end owns a rule of
//! its own: allocation, signals, fees and metrics all resolve to this crate.

pub mod audit;
pub mod backtest;
pub mod cli;
pub mod config;
pub mod db;
pub mod discipline;
pub mod guardrail;
// 数据集模块含完整的 CSV 元数据契约与可复算轨迹（抓取日、各腿起止、stale 天数、
// FGI 来源段等），部分字段供审计导出使用、当前打印层未全部消费。
#[allow(dead_code)]
pub mod live_data;
pub mod market;
pub mod metrics;
pub mod models;
pub mod quote;
pub mod report;
pub mod sentiment;
pub mod signal;
pub mod strategy;

/// Typed application services shared by both front ends.
pub mod app;

#[cfg(feature = "agent")]
pub mod agent;

#[cfg(feature = "gui")]
pub mod gui;
