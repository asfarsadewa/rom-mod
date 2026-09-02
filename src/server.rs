//! Local HTTP server hosting the workbench UI and the JSON API behind it.

use crate::cheatdb;
use crate::codes::Op;
use crate::patch::{self, Decoded};
use crate::rom::{self, Header, LibraryEntry, Platform, Rom, RomInfo};
use anyhow::{anyhow, Result};
use axum::{
    extract::{Path as UrlPath, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

pub struct AppState {
    pub roots: Vec<PathBuf>,
    pub library: Vec<LibraryEntry>,
    pub roms: HashMap<String, Arc<Rom>>,
    pub retroarch_dir: Option<PathBuf>,
}

type Shared = Arc<Mutex<AppState>>;

fn lock(s: &Shared) -> MutexGuard<'_, AppState> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

/// Find a RetroArch install by looking for retroarch.cfg in the usual places.
pub fn detect_retroarch() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from(r"C:\RetroArch-Win64"),
        PathBuf::from(r"C:\RetroArch"),
    ];
    for var in ["APPDATA", "LOCALAPPDATA", "HOME"] {
        if let Some(v) = std::env::var_os(var) {
            candidates.push(PathBuf::from(&v).join("RetroArch"));
            candidates.push(PathBuf::from(&v).join(".config").join("retroarch"));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.join("retroarch.cfg").is_file())
}

#[derive(Serialize)]
struct StateView {
    version: &'static str,
    roots: Vec<String>,
    retroarch_dir: Option<String>,
    library: Vec<LibraryEntry>,
}

fn view(s: &AppState) -> StateView {
    StateView {
        version: env!("CARGO_PKG_VERSION"),
        roots: s.roots.iter().map(|p| p.display().to_string()).collect(),
        retroarch_dir: s.retroarch_dir.as_ref().map(|p| p.display().to_string()),
        library: s.library.clone(),
    }
}

pub async fn serve(roots: Vec<PathBuf>, port: u16, open_browser: bool) -> Result<()> {
    let roots: Vec<PathBuf> = roots.into_iter().filter(|p| p.is_dir()).collect();
    let library = rom::scan(&roots);
    let state: Shared = Arc::new(Mutex::new(AppState {
        roots,
        library,
        roms: HashMap::new(),
        retroarch_dir: detect_retroarch(),
    }));
    let app = Router::new()
        .route("/", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/api/state", get(api_state))
        .route("/api/scan", post(api_scan))
        .route("/api/rom/:id", get(api_rom))
        .route("/api/rom/:id/cheats", get(api_cheats))
        .route("/api/rom/:id/decode", post(api_decode))
        .route("/api/rom/:id/build", post(api_build))
        .route("/api/rom/:id/cht", post(api_cht))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let url = format!("http://127.0.0.1:{port}/");
    eprintln!("rom-mod is listening on {url}");
    if open_browser {
        let _ = open::that(&url);
    }
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../ui/index.html"))
}

async fn css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../ui/app.css"),
    )
}

async fn js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../ui/app.js"),
    )
}

async fn api_state(State(s): State<Shared>) -> Json<StateView> {
    let l = lock(&s);
    Json(view(&l))
}

#[derive(Deserialize)]
struct ScanReq {
    root: Option<String>,
}

async fn api_scan(State(s): State<Shared>, Json(req): Json<ScanReq>) -> ApiResult<StateView> {
    if let Some(r) = req.root.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        let p = PathBuf::from(r);
        if !p.is_dir() {
            return Err(anyhow!("{} is not a folder", p.display()).into());
        }
        let mut l = lock(&s);
        if !l.roots.contains(&p) {
            l.roots.push(p);
        }
    }
    let roots = lock(&s).roots.clone();
    let library = tokio::task::spawn_blocking(move || rom::scan(&roots)).await?;
    let mut l = lock(&s);
    l.library = library;
    l.roms.clear();
    Ok(Json(view(&l)))
}

async fn get_rom(s: &Shared, id: &str) -> Result<Arc<Rom>> {
    if let Some(r) = lock(s).roms.get(id).cloned() {
        return Ok(r);
    }
    let entry = lock(s)
        .library
        .iter()
        .find(|e| e.id == id)
        .cloned()
        .ok_or_else(|| anyhow!("unknown ROM; rescan the library"))?;
    let path = PathBuf::from(&entry.path);
    let r = tokio::task::spawn_blocking(move || rom::load(&path)).await??;
    let r = Arc::new(r);
    lock(s).roms.insert(id.to_string(), r.clone());
    Ok(r)
}

#[derive(Serialize)]
struct RomView {
    id: String,
    name: String,
    platform: Platform,
    platform_label: &'static str,
    path: String,
    entry: Option<String>,
    info: RomInfo,
    header: Header,
}

async fn api_rom(State(s): State<Shared>, UrlPath(id): UrlPath<String>) -> ApiResult<RomView> {
    let r = get_rom(&s, &id).await?;
    Ok(Json(RomView {
        id: r.id.clone(),
        name: r.name.clone(),
        platform: r.platform,
        platform_label: r.platform.label(),
        path: r.path.display().to_string(),
        entry: r.entry.clone(),
        info: r.info.clone(),
        header: r.header.clone(),
    }))
}

#[derive(Serialize)]
pub struct CheatView {
    pub desc: String,
    pub code: String,
    pub parts: Vec<Decoded>,
    pub patchable: bool,
    pub runtime: bool,
    pub broken: bool,
    pub noop: bool,
}

pub fn classify(rom: &Rom, desc: &str, code: &str) -> CheatView {
    let parts = patch::decode(rom, code);
    let runtime = parts.iter().any(|p| matches!(p.op, Some(Op::Ram { .. })));
    let broken = parts.is_empty()
        || parts
            .iter()
            .any(|p| p.error.is_some() || (matches!(p.op, Some(Op::Rom { .. })) && !p.patchable));
    let patchable = !broken && !runtime && parts.iter().all(|p| p.patchable);
    let noop = patchable && parts.iter().all(|p| p.noop);
    CheatView {
        desc: desc.to_string(),
        code: code.to_string(),
        parts,
        patchable,
        runtime,
        broken,
        noop,
    }
}

#[derive(Deserialize)]
struct CheatsQuery {
    name: Option<String>,
}

#[derive(Serialize)]
struct CheatsView {
    source: &'static str,
    matched: Option<String>,
    candidates: Vec<String>,
    cheats: Vec<CheatView>,
    error: Option<String>,
}

async fn api_cheats(
    State(s): State<Shared>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<CheatsQuery>,
) -> ApiResult<CheatsView> {
    let rom = get_rom(&s, &id).await?;
    let platform = rom.platform;
    let name = rom.name.clone();
    let want = q.name.filter(|n| !n.trim().is_empty());
    let (matched, candidates, raw, error) = tokio::task::spawn_blocking(move || {
        let m = match cheatdb::find(platform, &name) {
            Ok(m) => m,
            Err(e) => return (None, Vec::new(), Vec::new(), Some(e.to_string())),
        };
        let chosen = want
            .or_else(|| m.exact.clone())
            .or_else(|| (m.candidates.len() == 1).then(|| m.candidates[0].clone()));
        match chosen {
            Some(n) => match cheatdb::fetch(platform, &n) {
                Ok(c) => (Some(n), m.candidates, c, None),
                Err(e) => (Some(n), m.candidates, Vec::new(), Some(e.to_string())),
            },
            None => (None, m.candidates, Vec::new(), None),
        }
    })
    .await?;
    let cheats = raw
        .iter()
        .map(|c| classify(&rom, &c.desc, &c.code))
        .collect();
    Ok(Json(CheatsView {
        source: "libretro-database",
        matched,
        candidates,
        cheats,
        error,
    }))
}

fn split_label(line: &str) -> (String, String) {
    let line = line.trim();
    match line.split_once('=') {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => (line.to_string(), line.to_string()),
    }
}

#[derive(Deserialize)]
struct DecodeReq {
    codes: Vec<String>,
}

async fn api_decode(
    State(s): State<Shared>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<DecodeReq>,
) -> ApiResult<Vec<CheatView>> {
    let rom = get_rom(&s, &id).await?;
    let out = req
        .codes
        .iter()
        .map(|line| split_label(line))
        .filter(|(_, code)| !code.is_empty())
        .map(|(desc, code)| classify(&rom, &desc, &code))
        .collect();
    Ok(Json(out))
}

#[derive(Deserialize)]
struct BuildReq {
    label: Option<String>,
    codes: Vec<String>,
    out_dir: Option<String>,
    #[serde(default)]
    overwrite: bool,
}

async fn api_build(
    State(s): State<Shared>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<BuildReq>,
) -> ApiResult<patch::BuildResult> {
    let rom = get_rom(&s, &id).await?;
    let label = req.label.unwrap_or_default();
    let out_dir = req
        .out_dir
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from);
    let codes = req.codes;
    let overwrite = req.overwrite;
    let res = tokio::task::spawn_blocking(move || {
        let ops = patch::collect_ops(&rom, &codes)?;
        patch::build(&rom, &ops, &label, out_dir.as_deref(), overwrite)
    })
    .await??;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct ChtItem {
    desc: String,
    code: String,
}

#[derive(Deserialize)]
struct ChtReq {
    core: String,
    cheats: Vec<ChtItem>,
    retroarch_dir: Option<String>,
}

#[derive(Serialize)]
struct ChtResult {
    path: String,
    count: usize,
}

async fn api_cht(
    State(s): State<Shared>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<ChtReq>,
) -> ApiResult<ChtResult> {
    let rom = get_rom(&s, &id).await?;
    let dir = req
        .retroarch_dir
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| lock(&s).retroarch_dir.clone())
        .ok_or_else(|| anyhow!("no RetroArch folder known; enter the one holding retroarch.cfg"))?;
    if !dir.join("retroarch.cfg").is_file() {
        return Err(anyhow!("{} has no retroarch.cfg", dir.display()).into());
    }
    let core = req.core.trim().to_string();
    if core.is_empty() {
        return Err(anyhow!("pick a core").into());
    }
    let cheats: Vec<(String, String)> = req.cheats.into_iter().map(|c| (c.desc, c.code)).collect();
    let count = cheats.len();
    let name = rom.name.clone();
    let path = tokio::task::spawn_blocking(move || {
        patch::write_retroarch_cht(&dir, &core, &name, &cheats)
    })
    .await??;
    lock(&s).retroarch_dir = Some(
        path.ancestors()
            .nth(3)
            .map(|p| p.to_path_buf())
            .unwrap_or_default(),
    );
    Ok(Json(ChtResult {
        path: path.display().to_string(),
        count,
    }))
}
