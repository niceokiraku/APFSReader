//! The window's behaviour against a scripted backend: no WinFsp, no disks.

use super::*;
use crate::backend::MountHandle;
use apfsreader_core::image::Kind;
use apfsreader_mount::MountInfo;
use std::collections::HashSet;

fn found(name: &str, role: Option<&'static str>, encrypted: bool) -> Found {
    Found {
        partition: Some(0),
        volume: Some(0),
        kind: Kind::Apfs,
        name: name.into(),
        role,
        encrypted,
        size: 1 << 20,
        problem: None,
    }
}

fn disk(number: u32, system: bool) -> DiskInfo {
    DiskInfo {
        number,
        vendor: "ACME".into(),
        model: format!("Stick {number}"),
        bus: "USB",
        size: 8_000_000_000,
        sector_size: 512,
        removable: true,
        is_system: system,
    }
}

struct FakeHandle(MountInfo);
impl MountHandle for FakeHandle {
    fn info(&self) -> &MountInfo {
        &self.0
    }
}

/// Behaviour keyed on the image file name, so each test says what it wants.
struct Fake {
    elevated: bool,
    winfsp: bool,
    disks: Vec<DiskInfo>,
    mounts_made: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn is_elevated(&self) -> bool {
        self.elevated
    }
    fn winfsp_available(&self) -> bool {
        self.winfsp
    }
    fn list_disks(&self) -> Vec<DiskInfo> {
        self.disks.clone()
    }
    fn free_letters(&self) -> Vec<String> {
        vec!["R:".into(), "S:".into()]
    }
    fn scan(&self, origin: &Origin) -> Result<ScanResult, String> {
        let found = match origin {
            Origin::Image(p) => match p.file_name().unwrap().to_str().unwrap() {
                "bad.dmg" => return Err("could not read the image".into()),
                "empty.dmg" => vec![],
                "enc.dmg" => vec![found("Locked", Some("data"), true)],
                "two.dmg" => vec![
                    found("Macintosh HD", Some("system"), false),
                    found("Macintosh HD - Data", Some("data"), false),
                ],
                "fail.dmg" => vec![found("Fail", None, false)],
                _ => vec![found("Photos", None, false)],
            },
            Origin::Disk(_) => vec![found("Backup", None, false)],
        };
        Ok(ScanResult { found, disk: None })
    }
    fn mount(
        &self,
        _o: &Origin,
        _d: Option<Arc<OpenDisk>>,
        f: &Found,
        mp: Option<String>,
    ) -> Result<Box<dyn MountHandle>, String> {
        if f.name == "Fail" {
            return Err("WinFsp refused".into());
        }
        let mount_point = mp.unwrap_or_else(|| "R:".into());
        self.mounts_made.lock().unwrap().push(f.name.clone());
        Ok(Box::new(FakeHandle(MountInfo { label: f.name.clone(), kind: f.kind, mount_point, notes: vec![] })))
    }
}

fn fake(elevated: bool, disks: Vec<DiskInfo>) -> Arc<Fake> {
    Arc::new(Fake { elevated, winfsp: true, disks, mounts_made: Mutex::new(vec![]) })
}

fn new_app(backend: Arc<Fake>, opts: Options) -> (App, egui::Context) {
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let app = App::new(ctx.clone(), backend, Arc::new(Shared::default()), opts);
    (app, ctx)
}

/// Run `logic` until `cond` holds (background threads deliver the events).
fn settle(app: &mut App, ctx: &egui::Context, cond: impl Fn(&App) -> bool) {
    for _ in 0..300 {
        app.logic(ctx);
        if cond(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the window never reached the expected state");
}

fn draw(app: &mut App, ctx: &egui::Context) {
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        app.show(ui);
        app.footer(ui);
    });
    // No renderer is attached, so the texture uploads have nowhere to go.
    output.textures_delta.clear();
    app.quit_dialog(ctx);
}

fn images(names: &[&str]) -> Options {
    Options {
        images: names.iter().map(|n| PathBuf::from(format!("C:\\imgs\\{n}"))).collect(),
        ..Default::default()
    }
}

fn scanned(app: &App) -> bool {
    app.cards.iter().all(|c| !matches!(c.scan, ScanState::Working))
}

#[test]
fn an_added_image_is_scanned_and_its_volume_listed() {
    let (mut app, ctx) = new_app(fake(false, vec![]), images(&["a.dmg"]));
    settle(&mut app, &ctx, scanned);
    assert_eq!(app.cards.len(), 1);
    assert!(matches!(&app.cards[0].scan, ScanState::Done { found, .. } if found.len() == 1));
    draw(&mut app, &ctx);
}

#[test]
fn mounting_a_row_creates_a_mount_and_unmounting_removes_it() {
    let backend = fake(false, vec![]);
    let (mut app, ctx) = new_app(backend.clone(), images(&["a.dmg"]));
    settle(&mut app, &ctx, scanned);
    let id = app.cards[0].id;
    app.start_mount(id, 0);
    settle(&mut app, &ctx, |a| a.shared.count() == 1);
    assert_eq!(backend.mounts_made.lock().unwrap().as_slice(), ["Photos"]);
    draw(&mut app, &ctx);
    let mount_id = app.shared.mounts.lock().unwrap()[0].id;
    app.unmount(mount_id);
    assert_eq!(app.shared.count(), 0);
    draw(&mut app, &ctx);
}

#[test]
fn auto_mount_picks_the_data_volume_not_the_system_one() {
    let backend = fake(false, vec![]);
    let opts = Options { auto_mount: true, ..images(&["two.dmg"]) };
    let (mut app, ctx) = new_app(backend.clone(), opts);
    settle(&mut app, &ctx, |a| a.shared.count() == 1);
    assert_eq!(backend.mounts_made.lock().unwrap().as_slice(), ["Macintosh HD - Data"]);
}

#[test]
fn a_failed_scan_is_shown_and_can_be_retried() {
    let (mut app, ctx) = new_app(fake(false, vec![]), images(&["bad.dmg"]));
    settle(&mut app, &ctx, scanned);
    assert!(matches!(&app.cards[0].scan, ScanState::Failed(m) if m.contains("could not read")));
    draw(&mut app, &ctx);
    let id = app.cards[0].id;
    app.start_scan(id);
    assert!(matches!(app.cards[0].scan, ScanState::Working));
    settle(&mut app, &ctx, scanned);
}

#[test]
fn an_image_with_nothing_mountable_says_so() {
    let (mut app, ctx) = new_app(fake(false, vec![]), images(&["empty.dmg"]));
    settle(&mut app, &ctx, scanned);
    assert!(matches!(&app.cards[0].scan, ScanState::Done { found, .. } if found.is_empty()));
    draw(&mut app, &ctx);
}

#[test]
fn an_encrypted_volume_is_listed_but_never_auto_mounted() {
    let opts = Options { auto_mount: true, ..images(&["enc.dmg"]) };
    let (mut app, ctx) = new_app(fake(false, vec![]), opts);
    settle(&mut app, &ctx, scanned);
    std::thread::sleep(Duration::from_millis(100));
    app.logic(&ctx);
    assert_eq!(app.shared.count(), 0);
    draw(&mut app, &ctx);
}

#[test]
fn a_mount_error_is_kept_on_its_row() {
    let (mut app, ctx) = new_app(fake(false, vec![]), images(&["fail.dmg"]));
    settle(&mut app, &ctx, scanned);
    let id = app.cards[0].id;
    app.start_mount(id, 0);
    settle(&mut app, &ctx, |a| !a.cards[0].rows[0].busy);
    assert_eq!(app.cards[0].rows[0].error.as_deref(), Some("WinFsp refused"));
    assert_eq!(app.shared.count(), 0);
    draw(&mut app, &ctx);
}

#[test]
fn the_same_image_is_not_added_twice() {
    let (mut app, ctx) = new_app(fake(false, vec![]), images(&["a.dmg"]));
    app.add_image(PathBuf::from("C:\\imgs\\a.dmg"));
    settle(&mut app, &ctx, scanned);
    assert_eq!(app.cards.iter().filter(|c| matches!(c.origin, Origin::Image(_))).count(), 1);
}

#[test]
fn disks_start_unread_and_scanning_one_is_an_explicit_action() {
    let (mut app, ctx) = new_app(fake(false, vec![disk(1, false), disk(0, true)]), Options::default());
    let numbers: HashSet<u32> = app
        .cards
        .iter()
        .filter_map(|c| if let Origin::Disk(d) = &c.origin { Some(d.number) } else { None })
        .collect();
    assert_eq!(numbers, HashSet::from([0, 1]));
    assert!(app.cards.iter().all(|c| matches!(c.scan, ScanState::Idle)), "nothing is read until asked");
    draw(&mut app, &ctx);
    let id = app.cards.iter().find(|c| matches!(&c.origin, Origin::Disk(d) if d.number == 1)).unwrap().id;
    app.start_scan(id);
    settle(&mut app, &ctx, scanned);
    let card = app.cards.iter().find(|c| c.id == id).unwrap();
    assert!(matches!(&card.scan, ScanState::Done { found, .. } if found.len() == 1));
}

#[test]
fn removing_a_source_takes_its_mounts_with_it() {
    let (mut app, ctx) = new_app(fake(false, vec![]), images(&["a.dmg", "b.dmg"]));
    settle(&mut app, &ctx, scanned);
    let ids: Vec<u64> = app.cards.iter().map(|c| c.id).collect();
    app.start_mount(ids[0], 0);
    app.start_mount(ids[1], 0);
    settle(&mut app, &ctx, |a| a.shared.count() == 2);
    let victim = ids[0];
    let doomed: Vec<u64> =
        app.shared.mounts.lock().unwrap().iter().filter(|m| m.source_id == victim).map(|m| m.id).collect();
    for id in doomed {
        app.unmount(id);
    }
    app.cards.retain(|c| c.id != victim);
    assert_eq!(app.shared.count(), 1);
    assert_eq!(app.cards.len(), 1);
}

#[test]
fn quitting_with_volumes_mounted_asks_first() {
    let (mut app, ctx) = new_app(fake(false, vec![]), images(&["a.dmg"]));
    settle(&mut app, &ctx, scanned);
    let id = app.cards[0].id;
    app.start_mount(id, 0);
    settle(&mut app, &ctx, |a| a.shared.count() == 1);
    app.request_quit(&ctx);
    assert!(app.confirm_quit && !app.quitting);
    draw(&mut app, &ctx);
    app.confirm_quit = false;
    app.shared.unmount_all();
    app.request_quit(&ctx);
    assert!(app.quitting, "with nothing mounted there is nothing to confirm");
}

#[test]
fn the_tooltip_count_reports_only_changes() {
    let (mut app, ctx) = new_app(fake(false, vec![]), images(&["a.dmg"]));
    assert_eq!(app.mount_count_changed(), Some(0));
    assert_eq!(app.mount_count_changed(), None);
    settle(&mut app, &ctx, scanned);
    let id = app.cards[0].id;
    app.start_mount(id, 0);
    settle(&mut app, &ctx, |a| a.shared.count() == 1);
    assert_eq!(app.mount_count_changed(), Some(1));
}

#[test]
fn an_elevated_session_still_lists_disks_as_unread() {
    let (app, _ctx) = new_app(fake(true, vec![disk(3, false)]), images(&["a.dmg"]));
    assert_eq!(app.card_count(), 2);
}

#[test]
fn the_physical_disks_section_starts_folded_and_the_light_theme_is_fixed() {
    let (mut app, ctx) = new_app(fake(false, vec![disk(1, false)]), Options::default());
    assert!(!app.disks_open, "folded by default");
    draw(&mut app, &ctx);
    app.disks_open = true;
    draw(&mut app, &ctx); // the unfolded layout draws too
    assert!(!ctx.global_style().visuals.dark_mode, "always light");
}

#[test]
fn the_bottom_strip_only_exists_while_there_is_a_message() {
    let (mut app, ctx) = new_app(fake(false, vec![]), images(&["a.dmg"]));
    assert!(!app.has_status(), "no standing hint text");
    settle(&mut app, &ctx, scanned);
    let id = app.cards[0].id;
    app.start_mount(id, 0);
    settle(&mut app, &ctx, |a| a.shared.count() == 1);
    assert!(app.has_status(), "mounting says where it went");
}

#[test]
fn the_about_box_carries_the_credit_winfsp_requires() {
    // WinFsp's licence (FLOSS exception) requires this notice and a link in the UI.
    let text = crate::app::view::about_text();
    assert!(text.contains("WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos"));
    assert!(text.contains("https://github.com/winfsp/winfsp"));
    assert!(text.contains("niceokiraku"));
    assert!(text.contains(env!("CARGO_PKG_VERSION")));
    let (mut app, ctx) = new_app(fake(false, vec![]), Options::default());
    app.show_about = true;
    draw(&mut app, &ctx);
}

#[test]
fn a_missing_winfsp_shows_a_banner_and_can_be_rechecked() {
    let backend = Arc::new(Fake { elevated: false, winfsp: false, disks: vec![], mounts_made: Mutex::new(vec![]) });
    let (mut app, ctx) = new_app(backend, Options::default());
    assert!(!app.winfsp_ok);
    draw(&mut app, &ctx); // the banner draws
    app.recheck_winfsp();
    assert!(!app.winfsp_ok, "still missing after a recheck");
}
