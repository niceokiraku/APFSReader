//! The notification-area (system tray) icon and its menu.
//!
//! The tray's callbacks run while the window may be hidden, when the window's
//! own update loop is not running, so they act directly: show the window via
//! viewport commands, or unmount and exit using the shared state.

use crate::app::Shared;
use crate::lang::t;
use eframe::egui::{self, ViewportCommand};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub struct Tray {
    icon: TrayIcon,
}

impl Tray {
    pub fn set_mounted(&self, n: usize) {
        let text = if n == 0 {
            "APFSReader".to_string()
        } else {
            format!("APFSReader — {n} {}", t("個マウント中", "mounted"))
        };
        let _ = self.icon.set_tooltip(Some(text));
    }
}

/// Shows balloon (toast) notifications from the tray icon.
///
/// `tray-icon` has no notification call, so this talks to the shell directly,
/// using the tray's own hidden window. The icon's id inside that window is not
/// exposed, so it is found by trying the few values the crate hands out: only
/// the real one makes the shell accept a change.
pub struct Notifier {
    hwnd: isize,
    uid: std::sync::atomic::AtomicU32,
    last: std::sync::Mutex<Option<std::time::Instant>>,
}

fn fill(dst: &mut [u16], s: &str) {
    // Leave room for the terminating zero.
    let room = dst.len().saturating_sub(1);
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(room)) {
        *d = c;
    }
}

impl Notifier {
    pub fn new(hwnd: isize) -> Self {
        Notifier { hwnd, uid: std::sync::atomic::AtomicU32::new(0), last: std::sync::Mutex::new(None) }
    }

    fn try_balloon(&self, uid: u32, title: &str, text: &str) -> bool {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::{
            Shell_NotifyIconW, NIF_INFO, NIIF_INFO, NIIF_NOSOUND, NIM_MODIFY, NOTIFYICONDATAW,
        };
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: HWND(self.hwnd as *mut _),
            uID: uid,
            uFlags: NIF_INFO,
            dwInfoFlags: NIIF_INFO | NIIF_NOSOUND,
            ..Default::default()
        };
        fill(&mut nid.szInfoTitle, title);
        fill(&mut nid.szInfo, text);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() }
    }

    /// The tray icon's id once a balloon has been accepted by the shell.
    pub fn accepted_uid(&self) -> Option<u32> {
        let u = self.uid.load(Ordering::Relaxed);
        (u != 0).then_some(u)
    }

    /// Show a balloon. Quietly does nothing if the shell refuses, and does not
    /// repeat itself within a few seconds (closing can also report a minimise).
    pub fn balloon(&self, title: &str, text: &str) {
        {
            let mut last = self.last.lock().unwrap();
            if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3)) {
                return;
            }
            *last = Some(std::time::Instant::now());
        }
        let known = self.uid.load(Ordering::Relaxed);
        if known != 0 && self.try_balloon(known, title, text) {
            return;
        }
        for uid in 1..=16 {
            if self.try_balloon(uid, title, text) {
                self.uid.store(uid, Ordering::Relaxed);
                return;
            }
        }
    }
}

/// Bring the window back from the tray.
pub fn show_window(ctx: &egui::Context, shared: &Shared) {
    shared.hidden.store(false, Ordering::Relaxed);
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
    ctx.request_repaint();
}

pub fn create(ctx: &egui::Context, shared: Arc<Shared>) -> Option<Tray> {
    let show = MenuItem::new(t("ウィンドウを表示", "Show window"), true, None);
    let unmount_all = MenuItem::new(t("すべてアンマウント", "Unmount all"), true, None);
    let quit = MenuItem::new(t("終了", "Quit"), true, None);
    let menu = Menu::new();
    menu.append(&show).ok()?;
    menu.append(&unmount_all).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit).ok()?;

    let rgba = apfsreader_icon::rgba(32);
    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("APFSReader")
        .with_icon(Icon::from_rgba(rgba, 32, 32).ok()?)
        .build()
        .ok()?;

    *shared.notifier.lock().unwrap() = Some(Notifier::new(tray_icon.window_handle() as isize));

    let (show_id, unmount_id, quit_id): (MenuId, MenuId, MenuId) =
        (show.id().clone(), unmount_all.id().clone(), quit.id().clone());

    {
        let (ctx, shared) = (ctx.clone(), shared.clone());
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| match e {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
            | TrayIconEvent::DoubleClick { .. } => show_window(&ctx, &shared),
            _ => {}
        }));
    }
    {
        let (ctx, shared) = (ctx.clone(), shared);
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == show_id {
                show_window(&ctx, &shared);
            } else if e.id == unmount_id {
                let (ctx, shared) = (ctx.clone(), shared.clone());
                std::thread::spawn(move || {
                    shared.unmount_all();
                    ctx.request_repaint();
                });
            } else if e.id == quit_id {
                // Unmount first so no drive is left pointing at a dead process.
                shared.unmount_all();
                std::process::exit(0);
            }
        }));
    }
    Some(Tray { icon: tray_icon })
}
