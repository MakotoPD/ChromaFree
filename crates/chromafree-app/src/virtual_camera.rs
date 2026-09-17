use anyhow::{Context, Result};
use chromafree_ipc::{VIRTUAL_CAMERA_CLSID, VIRTUAL_CAMERA_NAME};
use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MFCreateVirtualCamera, MFVirtualCameraAccess, MFVirtualCameraAccess_AllUsers,
    MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime, MFVirtualCameraLifetime_Session,
    MFVirtualCameraLifetime_System, MFVirtualCameraType_SoftwareCameraSource,
};
use windows::core::HSTRING;

fn create(lifetime: MFVirtualCameraLifetime, access: MFVirtualCameraAccess) -> Result<IMFVirtualCamera> {
    unsafe {
        MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            lifetime,
            access,
            &HSTRING::from(VIRTUAL_CAMERA_NAME),
            &HSTRING::from(VIRTUAL_CAMERA_CLSID),
            None,
        )
    }
    .context("creating the virtual camera")
}

pub struct VirtualCamera(IMFVirtualCamera);

impl VirtualCamera {
    pub fn create() -> Result<Self> {
        let camera = create(MFVirtualCameraLifetime_Session, MFVirtualCameraAccess_CurrentUser)?;
        unsafe { camera.Start(None) }.context("starting the virtual camera, is vcam-source.dll registered?")?;
        tracing::info!("session virtual camera registered");
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

pub fn install_system_camera() -> Result<()> {
    let camera = create(MFVirtualCameraLifetime_System, MFVirtualCameraAccess_AllUsers)?;
    unsafe { camera.Start(None) }.context("starting the system virtual camera, is vcam-source.dll registered?")?;
    tracing::info!("system virtual camera installed");
    Ok(())
}

pub fn uninstall_system_camera() -> Result<()> {
    let camera = create(MFVirtualCameraLifetime_System, MFVirtualCameraAccess_AllUsers)?;
    unsafe { camera.Remove() }.context("removing the system virtual camera")?;
    tracing::info!("system virtual camera removed");
    Ok(())
}
