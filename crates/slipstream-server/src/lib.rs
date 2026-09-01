//! Production HTTP boundary for the Slipstream Library core.
//!
//! This crate owns configuration, startup/shutdown, protocol mapping, and Web
//! delivery. It deliberately keeps filesystem indexing, persistence, and
//! Preview processing in `slipstream-core`.

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, Response, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Json},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use slipstream_core::{
    CacheDirectory, DerivativeTarget, Library, LibraryConfig, LibraryError, PhotoStateField,
    PhotoStateValue, PreviewCandidate, PreviewFacts, PreviewService, PreviewState, ScanLimits,
    ScanPhase, SelectionState, source_revision,
};
use std::{
    env,
    ffi::{CString, OsString},
    fmt, fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpListener,
    sync::{OnceCell, oneshot},
    task::JoinHandle,
};

mod app;
mod config;
pub(crate) mod folders;
mod http;
mod wire;

pub use app::Application;
#[cfg(test)]
pub(crate) use config::BROWSE_SNAPSHOT_IDLE;
pub use config::{Config, ConfigError, HEALTH_PATH, ServerConfig, ServerError, StartupConfig};
pub(crate) use config::{
    MAXIMUM_HEADER_BYTES, MAXIMUM_MUTATION_BODY_BYTES, validate_storage_layout,
};
#[cfg(test)]
pub(crate) use http::{HttpState, healthz, open_web_root, static_web};
pub use http::{RunningServer, create_router, expand_library, start_server};
pub use wire::{
    AlbumSummaryListResponse, AlbumSummaryWire, BrowseOpenResponse, BrowseSourceRequest,
    BrowseWindowResponse, DerivativeDelivery, FileLocationsResponse, FolderChildWire,
    LibraryOverviewResponse, OriginalWire, PhotoSummary, PreviewResponse, ScanStatusWire,
};
pub(crate) use wire::{album_summary, photo_summary_indexed_with_url, selection_state};

#[cfg(test)]
mod tests;
