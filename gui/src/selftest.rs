//! `--selftest`: drive the real window through minimise-to-tray and back,
//! and report through the exit code and a log file. This exists because the
//! risky part of tray behaviour is the event loop while the window is hidden,
//! which unit tests cannot reach and which needs a real window.

use crate::app::Shared;
use crate::single::WINDOW_TITLE;
use crate::tray;
use eframe::egui::{self, ViewportCommand};
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic, IsWindowVisible, PostMessageW, WM_CLOSE};

fn window() -> Option<HWND> {
    let title: Vec<u16> = WINDOW_TITLE.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())).ok() }
}

fn shown() -> bool {
    window().is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() })
}

fn hidden() -> bool {
    window().is_some_and(|h| unsafe { !IsWindowVisible(h).as_bool() })
}

/// Poll `cond` for up to `secs`.
fn wait_for(secs: u64, cond: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    cond()
}

/// `expect_mount`: an image was passed with `--mount`, so a drive must appear.
pub fn spawn(ctx: egui::Context, shared: Arc<Shared>, expect_mount: bool) {
    std::thread::spawn(move || {
        let mut log = std::fs::File::create(std::env::temp_dir().join("apfsreader-gui-selftest.log")).ok();
        let mut ok = true;
        let mut check = |name: &str, pass: bool| {
            ok &= pass;
            if let Some(f) = log.as_mut() {
                let _ = writeln!(f, "{} {name}", if pass { "ok  " } else { "FAIL" });
            }
        };

        check("window appears", wait_for(10, shown));

        // Minimise: the window must leave the screen and the taskbar (go to the tray).
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        ctx.request_repaint();
        check("minimising hides the window", wait_for(5, hidden));
        check("the hidden flag is set", shared.hidden.load(Ordering::Relaxed));

        // A balloon must be accepted by the shell (the tray icon id is found by probing).
        let accepted = shared.notifier.lock().unwrap().as_ref().is_some_and(|n| {
            n.balloon("APFSReader", "self-test notification");
            n.accepted_uid().is_some()
        });
        check("the shell accepts a tray balloon notification", accepted);

        // What the tray's Show does, called from another thread while the window is hidden.
        tray::show_window(&ctx, &shared);
        check("tray 'show' brings it back from hidden", wait_for(5, shown));
        check("the hidden flag is cleared", !shared.hidden.load(Ordering::Relaxed));

        // The close button must hide to the tray instead of exiting.
        if let Some(h) = window() {
            unsafe {
                let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        check("the close button hides to the tray", wait_for(5, hidden));
        check("the app is still running after close", window().is_some());

        tray::show_window(&ctx, &shared);
        check("shown again after close", wait_for(5, shown));

        if expect_mount {
            // The image given on the command line is scanned and mounted by the window itself.
            check("a volume gets mounted", wait_for(30, || shared.count() > 0));
            let mp = shared.mounts.lock().unwrap().first().map(|m| m.handle.info().mount_point.clone());
            let root = mp.clone().map(|m| format!("{m}\\"));
            let listed = root.as_ref().is_some_and(|r| std::fs::read_dir(r).is_ok_and(|d| d.count() > 0));
            check("the drive can be listed", listed);
            shared.unmount_all();
            let gone = root.as_ref().is_some_and(|r| wait_for(10, || !std::path::Path::new(r).exists()));
            check("the drive is gone after unmounting", gone);
        }

        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{}", if ok { "SELFTEST PASSED" } else { "SELFTEST FAILED" });
        }
        std::process::exit(if ok { 0 } else { 1 });
    });
}
