//! Everything the window asks the system to do, behind a trait so the screen
//! logic can be tested with a fake. The real implementation scans images and
//! disks, starts the elevated disk helper when a physical disk needs reading,
//! and mounts volumes through WinFsp.

use crate::lang::t;
use apfsreader_core::device::BlockSource;
use apfsreader_core::image::{self, Found};
use apfsreader_core::physical::{self, DiskInfo};
use apfsreader_core::remote::pipe::{connect_pipe, current_user_sid, is_user_cancelled, launch_broker, new_pipe_name, random_bytes};
use apfsreader_core::remote::pipe::BrokerProcess;
use apfsreader_mount::{MountInfo, MountRequest, Mounted, Source};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a source's data lives.
#[derive(Clone, Debug)]
pub enum Origin {
    Image(PathBuf),
    Disk(DiskInfo),
}

/// A physical disk opened for reading, possibly through the elevated helper.
/// Shared between all volumes mounted from the disk; when the last user lets
/// go, the pipe closes and the helper exits.
pub struct OpenDisk {
    source: Box<dyn BlockSource>,
    // Dropped after `source`, which closes the pipe the helper is waiting on.
    _helper: Option<BrokerProcess>,
}

struct SharedDisk(Arc<OpenDisk>);

impl BlockSource for SharedDisk {
    fn len(&self) -> u64 {
        self.0.source.len()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> apfsreader_core::Result<()> {
        self.0.source.read_at(offset, buf)
    }
    fn sector_size(&self) -> u64 {
        self.0.source.sector_size()
    }
}

/// What looking inside a source found.
pub struct ScanResult {
    pub found: Vec<Found>,
    /// The opened disk, kept for mounting its volumes.
    pub disk: Option<Arc<OpenDisk>>,
}

/// A mounted volume, as far as the window is concerned.
pub trait MountHandle: Send {
    fn info(&self) -> &MountInfo;
}

struct Real(Mounted);

impl MountHandle for Real {
    fn info(&self) -> &MountInfo {
        &self.0.info
    }
}

pub trait Backend: Send + Sync {
    fn is_elevated(&self) -> bool;
    /// Whether WinFsp is installed (without it nothing can be mounted).
    fn winfsp_available(&self) -> bool;
    fn list_disks(&self) -> Vec<DiskInfo>;
    fn free_letters(&self) -> Vec<String>;
    fn scan(&self, origin: &Origin) -> Result<ScanResult, String>;
    fn mount(
        &self,
        origin: &Origin,
        disk: Option<Arc<OpenDisk>>,
        found: &Found,
        mount_point: Option<String>,
    ) -> Result<Box<dyn MountHandle>, String>;
}

pub struct SystemBackend;

fn helper_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let p = exe.with_file_name("apfsreader-broker.exe");
    if p.is_file() {
        Ok(p)
    } else {
        Err(format!("{} {}", t("ディスクヘルパーが見つかりません:", "The disk helper is missing:"), p.display()))
    }
}

/// Turn a core error into something a person can act on.
pub fn friendly(e: &dyn std::fmt::Display) -> String {
    let s = e.to_string();
    let lower = s.to_lowercase();
    if lower.contains("encrypted") {
        return t(
            "このボリュームは暗号化されています。暗号化された APFS には対応していません。",
            "This volume is encrypted. Encrypted APFS is not supported.",
        )
        .to_string();
    }
    if lower.contains("winfsp is not installed") {
        return t(
            "WinFsp がインストールされていません。https://winfsp.dev から入手してください。",
            "WinFsp is not installed. Get it from https://winfsp.dev.",
        )
        .to_string();
    }
    s
}

fn open_disk_for_reading(disk: &DiskInfo) -> Result<Arc<OpenDisk>, String> {
    // Already elevated: read the disk directly.
    if physical::is_elevated() {
        let d = physical::open_disk(disk.number).map_err(|e| friendly(&e))?;
        return Ok(Arc::new(OpenDisk { source: Box::new(d), _helper: None }));
    }

    // Otherwise only the helper gets administrator rights, and the mount stays
    // with the user so the drive is visible in Explorer.
    let exe = helper_path()?;
    let pipe = new_pipe_name().map_err(|e| e.to_string())?;
    let token = random_bytes::<16>().map_err(|e| e.to_string())?;
    let sid = current_user_sid().map_err(|e| e.to_string())?;
    let helper = launch_broker(&exe, disk.number, &pipe, &token, &sid, 90).map_err(|e| {
        if is_user_cancelled(&e) {
            t("管理者権限の承認がキャンセルされました。", "Administrator approval was declined.").to_string()
        } else {
            e.to_string()
        }
    })?;

    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(code) = helper.exit_code() {
            return Err(match code {
                3 => t(
                    "ディスクを読み取り用に開けませんでした。他のプログラムが使用中か、権限が不足しています。",
                    "The disk could not be opened for reading. Another program may be using it, or access was refused.",
                )
                .to_string(),
                5 => t("ディスクヘルパーへの接続がタイムアウトしました。", "Connecting to the disk helper timed out.").to_string(),
                c => format!("{} ({c})", t("ディスクヘルパーが終了しました", "The disk helper exited")),
            });
        }
        match connect_pipe(&pipe, &token, Duration::from_millis(250)) {
            Ok(src) => return Ok(Arc::new(OpenDisk { source: Box::new(src), _helper: Some(helper) })),
            Err(apfsreader_core::Error::Format(m)) => return Err(m.to_string()),
            Err(e) if Instant::now() >= deadline => return Err(e.to_string()),
            Err(_) => {}
        }
    }
}

impl Backend for SystemBackend {
    fn is_elevated(&self) -> bool {
        physical::is_elevated()
    }

    fn winfsp_available(&self) -> bool {
        // `APFSREADER_SIMULATE_NO_WINFSP=1` pretends WinFsp is missing, to see the banner.
        if std::env::var_os("APFSREADER_SIMULATE_NO_WINFSP").is_some() {
            return false;
        }
        apfsreader_mount::ensure_winfsp().is_ok()
    }

    fn list_disks(&self) -> Vec<DiskInfo> {
        physical::list_disks()
    }

    fn free_letters(&self) -> Vec<String> {
        ('D'..='Z')
            .map(|c| format!("{c}:"))
            .filter(|d| !Path::new(&format!("{d}\\")).exists())
            .collect()
    }

    fn scan(&self, origin: &Origin) -> Result<ScanResult, String> {
        match origin {
            Origin::Image(path) => {
                let src = image::open_source(path).map_err(|e| friendly(&e))?;
                let found = image::discover(&*src).map_err(|e| friendly(&e))?;
                Ok(ScanResult { found, disk: None })
            }
            Origin::Disk(d) => {
                let disk = open_disk_for_reading(d)?;
                let found = image::discover(&SharedDisk(disk.clone())).map_err(|e| friendly(&e))?;
                Ok(ScanResult { found, disk: Some(disk) })
            }
        }
    }

    fn mount(
        &self,
        origin: &Origin,
        disk: Option<Arc<OpenDisk>>,
        found: &Found,
        mount_point: Option<String>,
    ) -> Result<Box<dyn MountHandle>, String> {
        let mount_point = match mount_point {
            Some(m) => m,
            None => apfsreader_mount::free_drive_letter()
                .ok_or_else(|| t("空いているドライブ文字がありません。", "No drive letter is free.").to_string())?,
        };
        let source = match (origin, disk) {
            (Origin::Image(p), _) => Source::Path(p.clone()),
            (Origin::Disk(_), Some(d)) => Source::Opened(Box::new(SharedDisk(d))),
            (Origin::Disk(_), None) => return Err("the disk is not open".into()),
        };
        let req = MountRequest { source, mount_point, partition: found.partition, volume: found.volume };
        apfsreader_mount::mount(req).map(|m| Box::new(Real(m)) as Box<dyn MountHandle>).map_err(|e| friendly(&e))
    }
}
