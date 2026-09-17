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
use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
use windows::core::{HRESULT, HSTRING, w};

pub struct SingleInstance(HANDLE);

impl SingleInstance {
    pub fn acquire() -> Result<Option<Self>> {
        let handle = unsafe { CreateMutexW(None, false, w!("Local\\bgcam-app-instance")) }
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
        .name("bgcam-config-watch".to_owned())
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

pub fn icon_rgba(size: u32) -> Vec<u8> {
    let center = size as f32 / 2.0;
    let outer = center - 0.5;
    let inner = outer * 0.45;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let distance = ((x as f32 + 0.5 - center).powi(2) + (y as f32 + 0.5 - center).powi(2)).sqrt();
            let coverage = (outer - distance + 0.5).clamp(0.0, 1.0);
            let pixel = if distance < inner {
                [24, 26, 30]
            } else if distance < inner + 1.5 {
                [230, 240, 235]
            } else {
                [0, 177, 64]
            };
            pixels.extend_from_slice(&[pixel[0], pixel[1], pixel[2], (coverage * 255.0) as u8]);
        }
    }
    pixels
}
