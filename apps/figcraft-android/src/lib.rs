//! FigCraft on Android, aimed at tablets with a keyboard, a stylus and a mouse.
//!
//! Android starts the app through `FigCraftActivity`, a `NativeActivity`
//! (packaging/android/java: it keeps the latest intent, colours the system bars and does the
//! updater's HTTP and install), which loads this library and calls [`android_main`]. The UI is the
//! desktop one (`figcraft-ui-egui`'s `FigApp`, in one window); what differs:
//! - Files: every file the UI asks for (Open, Import, Save As, Export, libraries, plugins…) is
//!   chosen in FigCraft's own file browser over shared storage (`files`), which needs "All files
//!   access" granted once in Settings (`file_access`).
//! - Settings and recovery copies live in the app's data folder. The recovery copies are the
//!   desktop app's (`apps/figcraft/src/recovery.rs`, compiled in here): unsaved files are copied
//!   every few seconds and when the app leaves the screen, and reopen on the next start when
//!   Android stopped the app in the background.
//! - Touch, S Pen, mouse, system bars and the keyboard cover (`tablet`, `touch`); the system
//!   clipboard's text through JNI (`clipboard`); documents from other apps ("Open with",
//!   `incoming`); new builds from the GitHub repository the APK was built from (`updater`).
//! - No `--control` channel, no other windows, and invite links aren't registered with Android.
//!
//! The approach (NativeActivity, the patched winit, JNI for what Rust can't reach) follows the
//! PdfCraft, VectorCraft, GridCraft and DesignCraft Android ports. Elsewhere the crate is empty,
//! so `cargo build --workspace` on a desktop still works.
#![cfg(target_os = "android")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod clipboard;
mod file_access;
mod files;
mod incoming;
#[path = "../../figcraft/src/recovery.rs"]
mod recovery;
mod tablet;
mod touch;
mod updater;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use figcraft_engine::{Prefs, Session};
use figcraft_ui_egui::{DialogFuture, FigApp, Services};
use serde_json::{Value, json};
use winit::platform::android::activity::AndroidApp;

use crate::files::{Kind, Picker};

/// Shared storage as apps see it (the device's "Internal storage").
pub(crate) const SHARED: &str = "/storage/emulated/0";

/// The app's internal data folder (settings, recovery, received documents).
static APP_DIR: OnceLock<PathBuf> = OnceLock::new();

pub(crate) fn app_dir() -> Option<PathBuf> {
    APP_DIR.get().cloned()
}

/// The settings folder (`<app data>/FigCraft`), as `~/.config/FigCraft` on Linux.
pub(crate) fn config_dir() -> Option<PathBuf> {
    app_dir().map(|d| d.join("FigCraft"))
}

/// The preferences' file in the settings folder, as on the desktop.
const STATE: &str = "state.json";

/// The preferences kept in `dir` (`None` on a first run).
fn load_prefs(dir: &Path) -> Option<Prefs> {
    let state: Value = serde_json::from_slice(&std::fs::read(dir.join(STATE)).ok()?).ok()?;
    let mut prefs = serde_json::from_value::<Prefs>(state.get("prefs")?.clone()).ok()?;
    prefs.recent_files.truncate(figcraft_engine::cmd::file::MAX_RECENT);
    Some(prefs)
}

/// Write the preferences atomically (a temporary file renamed over the old one: Android stopping
/// the app mid-write leaves the previous version whole).
pub(crate) fn write_prefs(dir: &Path, prefs: &Prefs) {
    let tmp = dir.join("state.json.tmp");
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, json!({"prefs": prefs}).to_string()))
        .and_then(|()| std::fs::rename(&tmp, dir.join(STATE)));
    if let Err(e) = written {
        log::warn!("preferences: couldn't save them in {}: {e}", dir.display());
    }
}

/// The file browser's shortcuts: the device's storage and its usual folders, SD cards and USB
/// drives that are plugged in, and FigCraft's own folder.
pub(crate) fn places() -> Vec<(String, PathBuf)> {
    let mut out = vec![
        ("Internal storage".to_string(), PathBuf::from(SHARED)),
        ("Documents".to_string(), PathBuf::from(SHARED).join("Documents")),
        ("Download".to_string(), PathBuf::from(SHARED).join("Download")),
        ("Pictures".to_string(), PathBuf::from(SHARED).join("Pictures")),
    ];
    if let Ok(entries) = std::fs::read_dir("/storage") {
        let mut volumes: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name != "emulated" && name != "self" && e.path().is_dir()
            })
            .map(|e| e.path())
            .collect();
        volumes.sort();
        for v in volumes {
            let name = v.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push((format!("SD card or USB drive ({name})"), v));
        }
    }
    if let Some(dir) = app_dir() {
        out.push(("App folder".to_string(), dir));
    }
    out
}

/// What the host gives the app on Android.
pub(crate) struct AndroidServices {
    picker: Picker,
    /// Where the preferences live ([`config_dir`]).
    dir: Option<PathBuf>,
    /// The recovery copies, written at once after a panic.
    recovery: recovery::Shared,
    /// The system's interface language (read at start-up).
    language: Option<String>,
}

impl Services for AndroidServices {
    fn pick_open(&mut self, title: &str, exts: &[&str]) -> DialogFuture {
        self.picker.request(Kind::Open, title, exts)
    }
    fn pick_save(&mut self, title: &str, name: &str, exts: &[&str]) -> DialogFuture {
        self.picker.request(Kind::Save(name.to_string()), title, exts)
    }
    fn pick_folder(&mut self, title: &str) -> DialogFuture {
        self.picker.request(Kind::Folder, title, &[])
    }
    fn clipboard_set_text(&mut self, s: &str) {
        clipboard::set_text(s);
    }
    fn clipboard_get_text(&mut self) -> Option<String> {
        clipboard::text()
    }
    fn open_url(&mut self, url: &str) {
        file_access::open_url(url);
    }
    fn save_prefs(&mut self, prefs: &Prefs) {
        if let Some(dir) = &self.dir {
            write_prefs(dir, prefs);
        }
    }
    fn emergency_save(&mut self, session: &Session) {
        if let Some(r) = recovery::lock(&self.recovery).as_mut() {
            r.write(session, true);
            r.settle(session);
        }
    }
    fn system_language(&self) -> Option<String> {
        self.language.clone()
    }
}

/// With nothing to open, whether FigCraft starts in a new file rather than on the home screen: a
/// first run (no settings kept yet) shows the home screen's tips and templates, later runs start
/// in a new file unless the preference asks for Home (as on the desktop).
fn opens_new_file(first_run: bool, start_on_home: bool) -> bool {
    !first_run && !start_on_home
}

/// Entry point, called by android-activity on its own thread once the activity is created.
#[allow(unsafe_code)] // Exported by name: android-activity looks this symbol up.
#[unsafe(no_mangle)]
fn android_main(android: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("FigCraft"));
    std::panic::set_hook(Box::new(|info| {
        let trace = std::backtrace::Backtrace::force_capture();
        log::error!("internal error: {info}\n{trace}");
    }));

    if let Some(dir) = android.internal_data_path() {
        let _ = APP_DIR.set(dir.clone());
        let tmp = dir.join("tmp");
        let _ = std::fs::create_dir_all(&tmp);
        // Android sets none of these. HOME is shared storage (the file browser's `~`, and a Fonts
        // folder there for the user's own fonts); settings go in the app's private folder,
        // temporary files where the app may write. Nothing else runs yet: this is the app's only
        // thread besides Java's.
        #[allow(unsafe_code)]
        // SAFETY: called before eframe or the app start any thread that could read the environment.
        unsafe {
            std::env::set_var("HOME", SHARED);
            std::env::set_var("XDG_CONFIG_HOME", &dir);
            std::env::set_var("XDG_DATA_HOME", &dir);
            std::env::set_var("TMPDIR", &tmp);
        }
        log::info!("FigCraft {} data folder {}", env!("CARGO_PKG_VERSION"), dir.display());
    }
    file_access::set_app(android.clone());
    log::info!("All files access: {}", if file_access::granted() { "granted" } else { "not granted" });
    let language = file_access::language();
    let shell_android = android.clone();

    // System fonts: index them in the background, then wake the UI (an idle window draws no frames)
    // to lay text out again with them.
    let ctx_slot: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
    let fonts_slot = ctx_slot.clone();
    let _ = std::thread::Builder::new().name("fonts".into()).spawn(move || {
        figcraft_text::scan_system_fonts();
        if let Some(c) = fonts_slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            c.request_repaint();
        }
    });

    let options = eframe::NativeOptions { android_app: Some(android), renderer: eframe::Renderer::Wgpu, ..Default::default() };
    let result = eframe::run_native(
        "FigCraft",
        options,
        Box::new(move |cc| {
            if let Some(state) = &cc.wgpu_render_state {
                let info = state.adapter.get_info();
                log::info!("graphics: wgpu on {} ({:?}, {:?})", info.name, info.device_type, info.backend);
            }
            *ctx_slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cc.egui_ctx.clone());
            let mut session = Session::empty();
            // Live sessions: the network threads wake the window when something arrives.
            let wake = cc.egui_ctx.clone();
            session.live.install(Box::new(figcraft_collab_net::NetLink::new(Arc::new(move || wake.request_repaint()))));
            let dir = config_dir();
            let prefs = dir.as_deref().and_then(load_prefs);
            let first_run = prefs.is_none();
            if let Some(prefs) = prefs {
                session.prefs = prefs;
            }
            // Documents a run Android stopped (or a crash) left unsaved come back; this run keeps
            // its own copies.
            let recovery_root = dir.as_ref().map(|d| d.join("Recovery"));
            let recovered = recovery_root.as_deref().map(|r| recovery::recover_all(r, &mut session)).unwrap_or_default();
            let mut copies = recovery_root.as_deref().and_then(recovery::Recovery::start);
            if let Some(r) = copies.as_mut() {
                r.adopt(recovered.waiting);
            }
            let recovery = recovery::Shared::new(Mutex::new(copies));
            // Plugins installed from a server's plugin directory live with the settings.
            session.plugins.install_dir = dir.as_ref().map(|d| d.join("Plugins"));
            if session.docs.is_empty() && opens_new_file(first_run, session.prefs.start_on_home) {
                let _ = session.execute("file.new", &json!({}));
            }
            figcraft_ui_egui::i18n::set(&session.prefs.language, language.as_deref());
            let picker = Picker::default();
            let services = AndroidServices { picker: picker.clone(), dir, recovery: recovery.clone(), language };
            let mut fig = FigApp::new(session, Box::new(services));
            fig.ui.home = false;
            // Android draws no window frame and FigCraft fills the screen: no window buttons.
            fig.ui.title_bar = figcraft_ui_egui::TitleBar::Native;
            fig.note_recovered(recovered.copies);
            figcraft_ui_egui::windows::install(fig);
            Ok(Box::new(tablet::TabletApp::new(picker, shell_android, recovery)))
        }),
    );
    if let Err(e) = result {
        log::error!("FigCraft stopped: {e}");
    }
}
