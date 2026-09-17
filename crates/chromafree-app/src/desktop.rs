use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindFirstChangeNotificationW,
    FindNextChangeNotification,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Threading::{CreateMutexW, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_RoamingAppData, FileOpenDialog, IFileOpenDialog, KF_FLAG_DEFAULT,
    SHGetKnownFolderPath, SIGDN_FILESYSPATH, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{GUID, HRESULT, HSTRING, w};

pub struct SingleInstance(HANDLE);

impl SingleInstance {
    pub fn acquire() -> Result<Option<Self>> {
        let handle = unsafe { CreateMutexW(None, false, w!("Local\\chromafree-app-instance")) }
            .context("creating the single instance mutex")?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            let _ = unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        Ok(Some(Self(handle)))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub fn pick_background_image() -> Result<Option<PathBuf>> {
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let dialog: IFileOpenDialog = unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }?;
    let filters = [COMDLG_FILTERSPEC {
        pszName: w!("Obrazy PNG i JPEG"),
        pszSpec: w!("*.png;*.jpg;*.jpeg"),
    }];
    unsafe {
        dialog.SetFileTypes(&filters)?;
        dialog.SetTitle(w!("Wybierz obraz tła"))?;
    }
    if let Err(error) = unsafe { dialog.Show(None) } {
        if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
            return Ok(None);
        }
        return Err(error.into());
    }
    let item = unsafe { dialog.GetResult() }?;
    let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }?;
    let path = unsafe { name.to_string() };
    unsafe { CoTaskMemFree(Some(name.0.cast())) };
    Ok(Some(PathBuf::from(path?)))
}

pub const PROJECT_URL: &str = env!("CARGO_PKG_REPOSITORY");
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_AUTHOR: &str = env!("CARGO_PKG_AUTHORS");

fn known_folder(id: &GUID) -> Option<PathBuf> {
    let path = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    let text = unsafe { path.to_string() };
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    text.ok().map(PathBuf::from)
}

pub fn log_directory() -> Option<PathBuf> {
    Some(known_folder(&FOLDERID_LocalAppData)?.join("ChromaFree"))
}

pub fn config_directory() -> Option<PathBuf> {
    Some(known_folder(&FOLDERID_RoamingAppData)?.join("ChromaFree"))
}

pub fn open_in_shell(target: &str) {
    let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL) };
    if result.0 as isize <= 32 {
        tracing::warn!(target, "opening failed");
    }
}

pub fn watch_directory(directory: &Path, on_change: impl Fn() + Send + 'static) -> Result<()> {
    let handle = unsafe {
        FindFirstChangeNotificationW(
            &HSTRING::from(directory),
            false,
            FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME,
        )
    }
    .with_context(|| format!("watching {}", directory.display()))?;
    let handle = handle.0 as isize;
    std::thread::Builder::new()
        .name("chromafree-config-watch".to_owned())
        .spawn(move || {
            let handle = HANDLE(handle as *mut _);
            while unsafe { WaitForSingleObject(handle, INFINITE) } == WAIT_OBJECT_0 {
                on_change();
                if unsafe { FindNextChangeNotification(handle) }.is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}
