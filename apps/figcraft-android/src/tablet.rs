//! What a tablet needs around the desktop UI. From the GridCraft, VectorCraft and DesignCraft
//! Android ports.
//!
//! - **System bars.** NativeActivity draws under the status bar and the taskbar. Android reports
//!   the part of the window they leave free (the content rect); the app is laid out inside it,
//!   and egui is told the same insets so dialogs and menus stay clear too. The bars take the tab
//!   strip's colour, with icons that show on it.
//! - **Touch** (`touch`): the S Pen and a mouse are the pointer; fingers tap, scroll and pinch.
//! - **Keyboard.** With a keyboard cover or Bluetooth keyboard attached, text fields don't ask
//!   for the on-screen keyboard.
//! - **Clipboard.** Text FigCraft copies goes on the system clipboard, and Ctrl+V pastes what
//!   other apps copied (`clipboard`).
//! - **Files** (`files`): the UI's file choices are answered by FigCraft's own browser.
//! - **Documents from other apps** open as they arrive (`incoming`).
//! - **Background.** Android may stop the app without warning once it leaves the screen, so the
//!   preferences and the recovery copies of unsaved files are written then.

use std::path::{Path, PathBuf};

use egui::Event;
use figcraft_ui_egui::FigApp;
use figcraft_ui_egui::theme::Tokens;
use figcraft_ui_egui::windows::{guard, with_app};
use serde_json::json;
use winit::platform::android::activity::AndroidApp;

use crate::files::{FileBrowser, Picker};
use crate::incoming::Incoming;
use crate::touch::TouchRouter;

pub struct TabletApp {
    /// File choices the UI asked for, answered one at a time by `browser`.
    picker: Picker,
    browser: Option<FileBrowser>,
    /// The folder the browser showed last (where it opens next time).
    last_dir: Option<PathBuf>,
    android: AndroidApp,
    touches: TouchRouter,
    /// The insets last applied, in points.
    insets: egui::epaint::MarginF32,
    /// The app's part of the screen last frame (clear of the system bars).
    inner: egui::Rect,
    /// Look for a document sent by another app (`incoming`): at start-up and when the app comes
    /// back to the front.
    check_incoming: bool,
    focused: bool,
    /// New builds from the GitHub repository the APK came from (`updater`).
    updater: crate::updater::Updater,
    /// The system bars' colour last sent to Android.
    bar_color: Option<egui::Color32>,
    /// Recovery copies of unsaved files (also written by the services after a panic).
    recovery: crate::recovery::Shared,
    paste: PasteBridge,
}

impl TabletApp {
    pub fn new(picker: Picker, android: AndroidApp, recovery: crate::recovery::Shared) -> Self {
        Self {
            picker,
            browser: None,
            last_dir: None,
            android,
            touches: TouchRouter::default(),
            insets: egui::epaint::MarginF32::default(),
            inner: egui::Rect::NOTHING,
            check_incoming: true,
            focused: true,
            updater: crate::updater::Updater::default(),
            bar_color: None,
            recovery,
            paste: PasteBridge::default(),
        }
    }

    /// Open the document another app sent, if there is one. One that only exists in the other
    /// app (an attachment, a cloud file) is first kept as a file of FigCraft's own, so Save writes
    /// somewhere real.
    fn open_incoming(fig: &mut FigApp) {
        let path = match crate::incoming::take() {
            None => return,
            Some(Incoming::Path(path)) => path,
            Some(Incoming::Bytes { name, bytes }) => match keep_copy(&name, &bytes) {
                Ok(path) => {
                    fig.toast(format!("Saved a copy of {name} in {}", path.parent().map_or_else(String::new, |p| p.display().to_string())));
                    path.to_string_lossy().into_owned()
                }
                Err(e) => {
                    fig.toast(format!("Couldn't open {name}: {e}"));
                    return;
                }
            },
            Some(Incoming::Failed(e)) => {
                fig.toast(e);
                return;
            }
        };
        log::info!("opening {path}");
        // The command brings a file that is already open to the front, and reports what fails.
        fig.ui.home = false;
        fig.run_user("file.open", json!({"path": path}));
    }

    /// Where the browser starts: where it was last, else the front file's folder, else Documents,
    /// else the app's folder.
    fn browser_start(&self, fig: &FigApp) -> PathBuf {
        let docs = PathBuf::from(crate::SHARED).join("Documents");
        self.last_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| fig.session.ds().ok().and_then(|d| d.path.as_deref()).map(Path::new).and_then(Path::parent).map(Path::to_path_buf).filter(|d| d.is_dir()))
            .or_else(|| Some(docs).filter(|d| d.is_dir() && crate::file_access::granted()))
            .or_else(crate::app_dir)
            .unwrap_or_else(|| PathBuf::from(crate::SHARED))
    }

    /// Show the next file choice the UI asked for, and hand the UI the answer once there is one.
    fn files(&mut self, ctx: &egui::Context) {
        if self.browser.is_none()
            && let Some(ask) = self.picker.next()
        {
            let start = with_app(|fig| self.browser_start(fig)).unwrap_or_else(|| PathBuf::from(crate::SHARED));
            self.browser = Some(FileBrowser::new(ask, start));
        }
        let Some(b) = self.browser.as_mut() else { return };
        if b.ask.abandoned() {
            self.browser = None;
            return;
        }
        let area = if self.inner.is_positive() { self.inner } else { ctx.content_rect() };
        let Some(answer) = b.show(ctx, area) else { return };
        self.last_dir = Some(b.dir.clone());
        b.ask.answer(answer);
        self.browser = None;
        ctx.request_repaint();
    }

    /// Colour the status bar and taskbar like the tab strip under them, with dark icons on a light
    /// colour and light ones on a dark colour.
    fn colour_system_bars(&mut self, color: egui::Color32) {
        if self.bar_color == Some(color) {
            return;
        }
        self.bar_color = Some(color);
        let [r, g, b, _] = color.to_array();
        let argb = i32::from_ne_bytes(u32::from_be_bytes([0xff, r, g, b]).to_ne_bytes());
        let luminance = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
        let light = luminance > 140.0;
        let _ = crate::file_access::with_activity("colouring the system bars", |env, activity| {
            env.call_method(activity, "fcSetSystemBars", "(IZ)V", &[jni::objects::JValue::Int(argb), jni::objects::JValue::Bool(u8::from(light))])?;
            Ok(())
        });
    }

    /// With a keyboard cover or Bluetooth keyboard attached, typing arrives as key presses, so a
    /// text field doesn't ask for the on-screen keyboard (Samsung shows its keyboard toolbar for
    /// that request, which takes room and comes and goes as fields gain and lose focus).
    fn keep_soft_keyboard_away(&self, ctx: &egui::Context) {
        if matches!(self.android.config().keys_hidden(), ndk::configuration::KeysHidden::No) {
            ctx.output_mut(|o| o.ime = None);
        }
    }

    /// The window's edges covered by system bars, in points, from Android's content rect.
    fn system_bar_insets(&self, ctx: &egui::Context, raw: &egui::RawInput) -> Option<egui::epaint::MarginF32> {
        let ppp = ctx.pixels_per_point();
        let screen = raw.screen_rect?;
        if !(ppp.is_finite() && ppp > 0.0) {
            return None;
        }
        let r = self.android.content_rect();
        if r.right <= r.left || r.bottom <= r.top {
            return None;
        }
        let (w, h) = (screen.width() * ppp, screen.height() * ppp);
        let inset = |px: f32| (px / ppp).clamp(0.0, 400.0);
        Some(egui::epaint::MarginF32 {
            left: inset(r.left as f32),
            top: inset(r.top as f32),
            right: inset(w - r.right as f32),
            bottom: inset(h - r.bottom as f32),
        })
    }

    /// Going to the background: keep the preferences, and the recovery copies of unsaved files
    /// (written now, not in the background: Android may stop the app any moment).
    fn to_background(&mut self) {
        with_app(|fig| {
            if let Some(dir) = crate::config_dir() {
                crate::write_prefs(&dir, &fig.session.prefs);
            }
            if let Some(r) = crate::recovery::lock(&self.recovery).as_mut() {
                r.write(&fig.session, true);
                r.settle(&fig.session);
            }
        });
    }
}

/// Ctrl+V pastes what other apps copied. egui-winit's own clipboard only knows FigCraft's copies:
/// it swallows the Ctrl+V press, sends a paste event only when its clipboard holds something, and
/// passes the release on, which FigCraft takes as Paste when no paste event came. Here a paste
/// event gets the system clipboard's text, and a Ctrl+V chord that brought none gets one with that
/// text just before its key event, so a text field pastes it too and the canvas pastes once.
#[derive(Default)]
struct PasteBridge {
    /// The Ctrl+V chord under way has had its paste event (it may span frames).
    pasted: bool,
    /// Cmd/Ctrl is held, as the last modifier change or key said.
    command: bool,
}

impl PasteBridge {
    fn route(&mut self, raw: &mut egui::RawInput, system_text: impl FnOnce() -> Option<String>) {
        let ctrl_v = |e: &Event| matches!(e, Event::Key { key: egui::Key::V, modifiers, .. } if modifiers.command && !modifiers.alt && !modifiers.shift);
        if !raw.events.iter().any(|e| matches!(e, Event::Paste(_)) || ctrl_v(e)) {
            if let Some(m) = raw.events.iter().rev().find_map(|e| match e {
                Event::ModifiersChanged(m) | Event::Key { modifiers: m, .. } => Some(m),
                _ => None,
            }) {
                self.command = m.command;
            }
            return;
        }
        let text = system_text();
        let mut out = Vec::with_capacity(raw.events.len() + 1);
        for mut e in std::mem::take(&mut raw.events) {
            if let Event::ModifiersChanged(m) | Event::Key { modifiers: m, .. } = &e {
                self.command = m.command;
            }
            if let Event::Paste(p) = &mut e {
                if let Some(t) = &text {
                    p.clone_from(t);
                }
                self.pasted |= self.command;
            } else if let Event::Key { pressed, .. } = e
                && ctrl_v(&e)
            {
                if !self.pasted
                    && let Some(t) = &text
                {
                    out.push(Event::Paste(t.clone()));
                }
                // The release ends the chord.
                self.pasted = pressed;
            }
            out.push(e);
        }
        raw.events = out;
    }
}

impl eframe::App for TabletApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(insets) = self.system_bar_insets(ctx, raw) {
            if insets != self.insets {
                log::info!("system bar insets (points): {insets:?}");
            }
            self.insets = insets;
            raw.safe_area_insets = Some(egui::SafeAreaInsets(insets));
        }
        if raw.focused && !self.focused {
            // Back in front, perhaps with another document to open.
            self.check_incoming = true;
        }
        if !raw.focused && self.focused {
            self.to_background();
        }
        self.focused = raw.focused;
        self.touches.route(&mut raw.events, raw.time.unwrap_or(0.0));
        if self.touches.waiting() {
            // A lift held back for the next frame, or a finger that may become a long press.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.paste.route(raw, crate::clipboard::text);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.check_incoming) {
            with_app(Self::open_incoming);
        }
        // At start-up, then at most hourly (`updater`).
        let now = ctx.input(|i| i.time);
        self.updater.maybe_check(now);
        // Copies of files with unsaved changes, every few seconds while they change.
        let recovery = &self.recovery;
        if let Some(wait) = with_app(|fig| crate::recovery::lock(recovery).as_mut().and_then(|r| r.tick(&fig.session, now))).flatten() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let full = ui.max_rect();
        let m = self.insets;
        let mut inner = egui::Rect::from_min_max(full.min + egui::vec2(m.left, m.top), full.max - egui::vec2(m.right, m.bottom));
        if inner.width() < 200.0 || inner.height() < 200.0 {
            inner = full;
        }
        self.inner = inner;
        // Behind the status bar and taskbar: the tab strip's colour, so the bars read as part of
        // the app, and Android draws their icons dark or light to match.
        let bar = Tokens::get(ui.ctx()).tab_bar;
        ui.painter().rect_filled(full, 0.0, bar);
        self.colour_system_bars(bar);
        let ctx = ui.ctx().clone();
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.set_clip_rect(inner);
            if with_app(|fig| guard(fig, &ctx, |fig| fig.ui(ui))).is_none() {
                log::warn!("the app is busy or not installed");
            }
        });
        self.files(&ctx);
        self.updater.show(&ctx, inner);
        // What FigCraft copied as text this frame (a text field, a layer's text) goes on the system
        // clipboard too.
        let copied = ctx.output(|o| {
            o.commands.iter().rev().find_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
        });
        if let Some(text) = copied {
            crate::clipboard::set_text(&text);
        }
        self.keep_soft_keyboard_away(&ctx);
    }

    fn on_exit(&mut self) {
        // As the desktop app quits: the preferences, this run's recovery copies (kept when the
        // unsaved-changes question wasn't asked), and the live sessions' ends.
        let copies = crate::recovery::lock(&self.recovery).take();
        with_app(|fig| {
            if let Some(dir) = crate::config_dir() {
                crate::write_prefs(&dir, &fig.session.prefs);
            }
            if let Some(r) = copies {
                r.quit(&fig.session, fig.ui.quit);
            }
            drop(std::mem::take(&mut fig.session.live));
        });
    }
}

/// Keep the bytes of a document that only exists in another app as a file FigCraft can open and
/// save: in Documents/FigCraft when shared storage is accessible, else in the app's own folder. An
/// existing file of that name is never replaced ("Poster (2).figcraft").
fn keep_copy(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let shared = PathBuf::from(crate::SHARED).join("Documents/FigCraft");
    let dir = if crate::file_access::granted() { shared } else { crate::app_dir().map(|d| d.join("Received")).ok_or("no app folder")? };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = free_name(&dir, &safe_file_name(name));
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// `name` without folders or characters a file name can't hold; never empty.
fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String =
        base.chars().map(|c| if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() { "Untitled.figcraft".to_string() } else { cleaned }
}

/// `dir/name`, or `dir/stem (2).ext` and so on when that file already exists.
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_of_incoming_documents_get_free_names() {
        assert_eq!(safe_file_name("../a/b:c.fig"), "b_c.fig");
        assert_eq!(safe_file_name(""), "Untitled.figcraft");
        let dir = std::env::temp_dir().join(format!("figcraft-android-free-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("R.fig"), b"x").unwrap();
        assert_eq!(free_name(&dir, "R.fig"), dir.join("R (2).fig"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn key_v(pressed: bool) -> Event {
        Event::Key {
            key: egui::Key::V,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers { command: true, ctrl: true, ..Default::default() },
        }
    }

    fn frame(mut events: Vec<Event>, command: bool) -> egui::RawInput {
        events.insert(0, Event::ModifiersChanged(egui::Modifiers { command, ctrl: command, ..Default::default() }));
        egui::RawInput { events, ..Default::default() }
    }

    /// The events after the leading modifier change [`frame`] adds.
    fn rest(raw: &egui::RawInput) -> &[Event] {
        raw.events.get(1..).unwrap_or_default()
    }

    #[test]
    fn a_ctrl_v_chord_pastes_the_system_text_once() {
        let sys = || Some("from mail".to_string());
        // egui-winit swallowed the press (nothing of its own to paste): the release gets the paste.
        let mut b = PasteBridge::default();
        let mut raw = frame(vec![key_v(false)], false);
        b.route(&mut raw, sys);
        assert_eq!(rest(&raw), [Event::Paste("from mail".into()), key_v(false)]);
        // egui-winit pasted its own copy on the press: that paste takes the system text, and the
        // release a frame later adds nothing.
        let mut raw = frame(vec![Event::Paste("ours".into())], true);
        b.route(&mut raw, sys);
        assert_eq!(rest(&raw), [Event::Paste("from mail".into())]);
        let mut raw = frame(vec![key_v(false)], false);
        b.route(&mut raw, sys);
        assert_eq!(rest(&raw), [key_v(false)]);
        // The next chord pastes again.
        let mut raw = frame(vec![key_v(true), key_v(false)], true);
        b.route(&mut raw, sys);
        assert_eq!(rest(&raw), [Event::Paste("from mail".into()), key_v(true), key_v(false)]);
        // Nothing on the system clipboard: unchanged.
        let mut raw = frame(vec![key_v(false)], false);
        b.route(&mut raw, || None);
        assert_eq!(rest(&raw), [key_v(false)]);
    }
}
