use anyhow::{Context, Result};
use bgcam_ipc::{VIRTUAL_CAMERA_CLSID, VIRTUAL_CAMERA_NAME};
use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MFCreateVirtualCamera, MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_Session,
    MFVirtualCameraType_SoftwareCameraSource,
};
use windows::core::HSTRING;

pub struct VirtualCamera(IMFVirtualCamera);

impl VirtualCamera {
    pub fn create() -> Result<Self> {
        let camera = unsafe {
            MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                &HSTRING::from(VIRTUAL_CAMERA_NAME),
                &HSTRING::from(VIRTUAL_CAMERA_CLSID),
                None,
            )
        }
        .context("creating the virtual camera")?;
        unsafe { camera.Start(None) }.context("starting the virtual camera, is vcam-source.dll registered?")?;
        tracing::info!("virtual camera registered");
        Ok(Self(camera))
    }
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        if let Err(error) = unsafe { self.0.Remove() } {
            tracing::warn!(%error, "removing the virtual camera failed");
        }
    }
}
