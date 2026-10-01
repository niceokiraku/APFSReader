//! The window: a list of sources (image files and physical disks), the volumes
//! found in each, and what is currently mounted.

use crate::backend::{Backend, MountHandle, Origin, OpenDisk, ScanResult};
use crate::lang::t;
use apfsreader_core::image::{self, Found};
use apfsreader_core::physical::DiskInfo;
use eframe::egui::{self, ViewportCommand};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the disk list is refreshed while the window is open.
const DISK_REFRESH: Duration = Duration::from_secs(3);
const STATUS_TTL: Duration = Duration::from_secs(10);

/// A volume that is mounted, and which row of which source it came from.
pub struct Mount {
    pub id: u64,
    pub source_id: u64,
    pub row: usize,
    pub handle: Box<dyn MountHandle>,
}

/// State shared with the tray icon, whose callbacks run outside the window code.
#[derive(Default)]
pub struct Shared {
    pub mounts: Mutex<Vec<Mount>>,
    /// True while the window is hidden in the tray.
    pub hidden: AtomicBool,
    /// Raises balloon notifications from the tray icon, once it exists.
    pub notifier: Mutex<Option<crate::tray::Notifier>>,
    /// Suppresses balloons (used by the self-test, which hides the window on purpose).
    pub quiet: AtomicBool,
}

impl Shared {
    /// Unmount everything, blocking until it is done.
    pub fn unmount_all(&self) {
        let all: Vec<Mount> = std::mem::take(&mut *self.mounts.lock().unwrap());
        drop(all); // each handle unmounts and releases its image or disk
    }

    pub fn count(&self) -> usize {
        self.mounts.lock().unwrap().len()
    }
}

enum ScanState {
    /// A disk not yet read: reading it may need administrator approval.
    Idle,
    Working,
    Done { found: Vec<Found>, disk: Option<Arc<OpenDisk>> },
    Failed(String),
}

#[derive(Default, Clone)]
struct RowUi {
    /// `None` means the next free letter.
    letter: Option<String>,
    busy: bool,
    error: Option<String>,
}

struct Card {
    id: u64,
    origin: Origin,
    scan: ScanState,
    rows: Vec<RowUi>,
}

enum Event {
    Scanned(u64, Result<ScanResult, String>),
    Mounted(u64, usize, Result<Box<dyn MountHandle>, String>),
    Unmounted,
}

/// Start-up options from the command line.
#[derive(Default, Clone)]
pub struct Options {
    pub images: Vec<PathBuf>,
    /// Mount the recommended volume of each added image as soon as it is scanned.
    pub auto_mount: bool,
    /// Open the About box at start-up.
    pub show_about: bool,
}

pub struct App {
    ctx: egui::Context,
    /// The app icon, shown in the header.
    icon: egui::TextureHandle,
    backend: Arc<dyn Backend>,
    pub shared: Arc<Shared>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    cards: Vec<Card>,
    next_id: u64,
    disks_seen: Instant,
    elevated: bool,
    free_letters: Vec<String>,
    show_system_disk: bool,
    /// Whether the physical-disks section is unfolded (folded by default).
    disks_open: bool,
    /// False when WinFsp is not installed.
    winfsp_ok: bool,
    show_about: bool,
    status: Option<(String, Instant)>,
    auto_mount: bool,
    confirm_quit: bool,
    pub quitting: bool,
    last_tooltip_count: usize,
    was_minimized: bool,
}

impl App {
    pub fn new(ctx: egui::Context, backend: Arc<dyn Backend>, shared: Arc<Shared>, opts: Options) -> Self {
        let (tx, rx) = channel();
        let icon = ctx.load_texture(
            "app-icon",
            egui::ColorImage::from_rgba_unmultiplied([96, 96], &apfsreader_icon::rgba(96)),
            egui::TextureOptions::LINEAR,
        );
        let mut app = App {
            icon,
            ctx,
            elevated: backend.is_elevated(),
            free_letters: backend.free_letters(),
            backend,
            shared,
            tx,
            rx,
            cards: Vec::new(),
            next_id: 1,
            disks_seen: Instant::now() - DISK_REFRESH,
            show_system_disk: false,
            disks_open: false,
            winfsp_ok: true,
            show_about: opts.show_about,
            status: None,
            auto_mount: opts.auto_mount,
            confirm_quit: false,
            quitting: false,
            last_tooltip_count: usize::MAX,
            was_minimized: false,
        };
        app.winfsp_ok = app.backend.winfsp_available();
        for p in opts.images {
            app.add_image(p);
        }
        app.refresh_disks();
        app
    }

    /// Look for WinFsp again (it may have been installed since the window opened).
    fn recheck_winfsp(&mut self) {
        self.winfsp_ok = self.backend.winfsp_available();
    }

    // ---- actions -------------------------------------------------------

    fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn add_image(&mut self, path: PathBuf) {
        let same = |c: &Card| matches!(&c.origin, Origin::Image(p) if *p == path);
        if self.cards.iter().any(same) {
            self.say(t("このイメージはすでに追加されています。", "That image is already in the list."));
            return;
        }
        let id = self.alloc_id();
        self.cards.insert(0, Card { id, origin: Origin::Image(path), scan: ScanState::Working, rows: vec![] });
        self.start_scan(id);
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn start_scan(&mut self, id: u64) {
        let Some(card) = self.cards.iter_mut().find(|c| c.id == id) else { return };
        card.scan = ScanState::Working;
        let origin = card.origin.clone();
        let (backend, tx, ctx) = (self.backend.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = backend.scan(&origin);
            let _ = tx.send(Event::Scanned(id, result));
            ctx.request_repaint();
        });
    }

    fn start_mount(&mut self, id: u64, row: usize) {
        let Some(card) = self.cards.iter_mut().find(|c| c.id == id) else { return };
        let ScanState::Done { found, disk } = &card.scan else { return };
        let (Some(f), Some(r)) = (found.get(row).cloned(), card.rows.get_mut(row)) else { return };
        r.busy = true;
        r.error = None;
        let (origin, disk, letter) = (card.origin.clone(), disk.clone(), r.letter.clone());
        let (backend, tx, ctx) = (self.backend.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = backend.mount(&origin, disk, &f, letter);
            let _ = tx.send(Event::Mounted(id, row, result));
            ctx.request_repaint();
        });
    }

    fn unmount(&mut self, mount_id: u64) {
        let taken = {
            let mut m = self.shared.mounts.lock().unwrap();
            m.iter().position(|x| x.id == mount_id).map(|i| m.remove(i))
        };
        if let Some(mount) = taken {
            let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
            // Stopping WinFsp can take a moment; keep the window responsive.
            std::thread::spawn(move || {
                drop(mount);
                let _ = tx.send(Event::Unmounted);
                ctx.request_repaint();
            });
        }
    }

    fn refresh_disks(&mut self) {
        self.disks_seen = Instant::now();
        self.free_letters = self.backend.free_letters();
        let disks = self.backend.list_disks();
        // Drop disks that are gone, unless something is still mounted from them.
        let mounted_sources: Vec<u64> = self.shared.mounts.lock().unwrap().iter().map(|m| m.source_id).collect();
        self.cards.retain(|c| match &c.origin {
            Origin::Disk(d) => disks.iter().any(|n| n.number == d.number) || mounted_sources.contains(&c.id),
            Origin::Image(_) => true,
        });
        for d in disks {
            let known = self.cards.iter().any(|c| matches!(&c.origin, Origin::Disk(k) if k.number == d.number));
            if !known {
                let id = self.alloc_id();
                self.cards.push(Card { id, origin: Origin::Disk(d), scan: ScanState::Idle, rows: vec![] });
            }
        }
    }

    fn poll_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Scanned(id, result) => self.on_scanned(id, result),
                Event::Mounted(id, row, result) => self.on_mounted(id, row, result),
                Event::Unmounted => {}
            }
        }
    }

    fn on_scanned(&mut self, id: u64, result: Result<ScanResult, String>) {
        let auto = self.auto_mount;
        let Some(card) = self.cards.iter_mut().find(|c| c.id == id) else { return };
        match result {
            Ok(ScanResult { found, disk }) => {
                card.rows = vec![RowUi::default(); found.len()];
                let pick = if auto && matches!(card.origin, Origin::Image(_)) { image::preferred(&found) } else { None };
                card.scan = ScanState::Done { found, disk };
                if let Some(row) = pick {
                    self.start_mount(id, row);
                }
            }
            Err(e) => card.scan = ScanState::Failed(e),
        }
    }

    fn on_mounted(&mut self, id: u64, row: usize, result: Result<Box<dyn MountHandle>, String>) {
        let Some(card) = self.cards.iter_mut().find(|c| c.id == id) else { return };
        if let Some(r) = card.rows.get_mut(row) {
            r.busy = false;
        }
        match result {
            Ok(handle) => {
                let note = handle.info().notes.first().cloned();
                let mount_id = self.next_id + 1;
                self.next_id += 1;
                let msg = if crate::lang::japanese() {
                    format!("{} にマウントしました", handle.info().mount_point)
                } else {
                    format!("Mounted at {}", handle.info().mount_point)
                };
                self.shared.mounts.lock().unwrap().push(Mount { id: mount_id, source_id: id, row, handle });
                self.say(match note {
                    Some(n) => format!("{msg}  ({n})"),
                    None => msg,
                });
            }
            Err(e) => {
                if let Some(r) = card.rows.get_mut(row) {
                    r.error = Some(e);
                }
            }
        }
    }

    // ---- window behaviour ----------------------------------------------

    /// Minimise and the close button both move the window to the tray; the
    /// tray menu (or the Quit button) is what actually exits.
    fn handle_window(&mut self, ctx: &egui::Context) {
        let (minimized, close_requested) =
            ctx.input(|i| (i.viewport().minimized == Some(true), i.viewport().close_requested()));
        if close_requested && !self.quitting {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide_to_tray(ctx);
        } else if minimized && !self.was_minimized {
            // Only the moment the window becomes minimised counts. Right after a
            // tray "show" the state can still read minimised for a frame, and
            // acting on that would hide the window again.
            self.hide_to_tray(ctx);
        }
        self.was_minimized = minimized;
    }

    pub fn hide_to_tray(&mut self, ctx: &egui::Context) {
        self.shared.hidden.store(true, Ordering::Relaxed);
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        // Say where the app went: it is still running, and how to get it back.
        if !self.shared.quiet.load(Ordering::Relaxed) {
            if let Some(n) = self.shared.notifier.lock().unwrap().as_ref() {
                n.balloon(
                    "APFSReader",
                    t(
                        "システムトレイで動作を続けています。アイコンをクリックすると開きます。",
                        "Still running in the system tray. Click the icon to open it.",
                    ),
                );
            }
        }
    }

    fn request_quit(&mut self, ctx: &egui::Context) {
        if self.shared.count() > 0 {
            self.confirm_quit = true;
        } else {
            self.quit(ctx);
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        self.shared.unmount_all();
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// Number of mounted volumes, for the tray tooltip. `None` if unchanged.
    pub fn mount_count_changed(&mut self) -> Option<usize> {
        let n = self.shared.count();
        (n != self.last_tooltip_count).then(|| {
            self.last_tooltip_count = n;
            n
        })
    }

    // ---- drawing -------------------------------------------------------

    fn mount_of(&self, card_id: u64, row: usize) -> Option<(u64, String, String)> {
        let m = self.shared.mounts.lock().unwrap();
        m.iter()
            .find(|x| x.source_id == card_id && x.row == row)
            .map(|x| (x.id, x.handle.info().mount_point.clone(), x.handle.info().label.clone()))
    }

    /// Per-frame work: events, disk polling, drops, window state.
    pub fn logic(&mut self, ctx: &egui::Context) {
        self.poll_events();
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for p in dropped {
            self.add_image(p);
        }
        if self.disks_seen.elapsed() >= DISK_REFRESH && !self.shared.hidden.load(Ordering::Relaxed) {
            self.refresh_disks();
        }
        if let Some((_, at)) = &self.status {
            if at.elapsed() > STATUS_TTL {
                self.status = None;
            }
        }
        self.handle_window(ctx);
        ctx.request_repaint_after(DISK_REFRESH);
    }
}

pub mod view;

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
