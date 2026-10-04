use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use reqwest::{Client, Method, StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    env,
    ffi::OsString,
    fmt,
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

mod compatibility;
mod development;
mod development_proxy;
mod edit;
mod historical_export_download;
mod preview_download;
mod processing_artifact_download;
mod processing_preview_download;

const CLI_CONTRACT_VERSION: u16 = 1;
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_LIST_PAGE: usize = 50;
const MAXIMUM_LIST_PAGE: usize = 60;
const CONTRACT_HEADER: &str = "Slipstream-CLI-Contract";
const MAXIMUM_JSON_RESPONSE_BYTES: usize = 1024 * 1024;
const MAXIMUM_INPUT_BYTES: usize = 64 * 1024;
const MAXIMUM_MUTATION_PHOTO_IDS: usize = 100;
const MAXIMUM_RECOVERY_APPLY: usize = 100;
const MAXIMUM_TRASH_IDS: usize = 5_000;
pub(crate) const MAXIMUM_SOURCE_REVISION_BYTES: usize = 16_384;
mod client;
mod commands;
mod execute;
mod input;
mod operations;
mod protocol;
mod state;
#[cfg(test)]
mod tests;

pub(crate) use client::*;
pub(crate) use commands::*;
pub use commands::{
    AlbumCommand, AlbumListArgs, AlbumMembershipArgs, Cli, Command, FolderCommand, FolderListArgs,
    InvocationResult, LibraryCommand, OrderArg, OriginalKindArg, OutputFormat,
    ParseErrorPreferences, PhotoCommand, PhotoDecisionArgs, PhotoListArgs, PhotoMetadataSaveArgs,
    PhotoRemovalArgs, PhotoRestoreArgs, PreviewSize, ProcessingCommand, RecoveryApplyArgs,
    RecoveryCommand, RecoveryProposeArgs, RecoveryUnavailableArgs, SelectionArg, SetSelectionArg,
    TrashCommand, TrashListArgs, TrashReviewArgs, parse_error_preferences,
};
pub(crate) use execute::*;
pub use execute::{invalid_invocation, invoke, invoke_until};
pub(crate) use input::*;
pub(crate) use operations::*;
pub(crate) use protocol::*;
pub(crate) use state::*;
