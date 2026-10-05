//! Windows GDI gamma control. No calls are made from the frame loop.
use super::guardian::{device_key, Backend, Device, Owner, Ramp};
use std::{mem::size_of, path::Path};
use windows_sys::Win32::{
    Devices::Display::*,
    Foundation::*,
    Graphics::Gdi::*,
    Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
    System::Threading::*,
    UI::{
        ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp},
        WindowsAndMessaging::GetWindowThreadProcessId,
    },
};
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}
fn text(value: &[u16]) -> String {
    String::from_utf16_lossy(&value[..value.iter().position(|v| *v == 0).unwrap_or(value.len())])
}
fn os_error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
}

pub(super) struct Lease(HANDLE);
impl Drop for Lease {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}
pub(super) fn monitor_lock(device: &Device) -> Result<Lease, String> {
    let name = wide(&format!("Local\\DinurdoJK.Gamma.{}", device_key(device)));
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(os_error("create gamma ownership mutex"));
    }
    let result = unsafe { WaitForSingleObject(handle, 0) };
    if result == WAIT_OBJECT_0 || result == WAIT_ABANDONED {
        Ok(Lease(handle))
    } else {
        unsafe {
            CloseHandle(handle);
        }
        Err("display gamma is owned by another running game".into())
    }
}
fn birth(handle: HANDLE) -> Result<u64, String> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(os_error("get gamma process identity"));
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}
pub(super) fn own_identity() -> Result<Owner, String> {
    Ok(Owner {
        pid: unsafe { GetCurrentProcessId() },
        born: birth(unsafe { GetCurrentProcess() })?,
    })
}
pub(super) fn wait_parent(owner: Owner) -> Result<(), String> {
    let handle = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            owner.pid,
        )
    };
    if handle.is_null() {
        return Err(os_error("open gamma parent process"));
    }
    let result = (|| {
        if birth(handle)? != owner.born {
            return Err("gamma parent process identity changed".into());
        }
        let result = unsafe { WaitForSingleObject(handle, INFINITE) };
        if result == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(os_error("wait for gamma parent exit"))
        }
    })();
    unsafe {
        CloseHandle(handle);
    }
    result
}
pub(super) fn parent_alive(owner: Owner) -> bool {
    let handle = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            owner.pid,
        )
    };
    if handle.is_null() {
        return false;
    }
    let alive = birth(handle).ok() == Some(owner.born)
        && unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    unsafe {
        CloseHandle(handle);
    }
    alive
}
fn device_identity(name: &str) -> Result<String, String> {
    let name = wide(name);
    let mut device = DISPLAY_DEVICEW::default();
    device.cb = size_of::<DISPLAY_DEVICEW>() as u32;
    if unsafe { EnumDisplayDevicesW(name.as_ptr(), 0, &mut device, 0) } == 0 {
        return Err(os_error("identify gamma monitor"));
    }
    let id = text(&device.DeviceID);
    if id.is_empty() {
        return Err("monitor has no stable gamma recovery identity".into());
    }
    Ok(id)
}
fn check_sdr(name: &str) -> Result<(), String> {
    let mut path_count = 0;
    let mut mode_count = 0;
    let flags = QDC_ONLY_ACTIVE_PATHS;
    let error = unsafe { GetDisplayConfigBufferSizes(flags, &mut path_count, &mut mode_count) };
    if error != ERROR_SUCCESS {
        return Err(format!(
            "cannot query display color mode: Windows error {error}"
        ));
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
    let error = unsafe {
        QueryDisplayConfig(
            flags,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    if error != ERROR_SUCCESS {
        return Err(format!(
            "cannot query display color mode: Windows error {error}"
        ));
    }
    let mut found = false;
    for path in paths.iter().take(path_count as usize) {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        source.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        };
        if unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } != 0
            || text(&source.viewGdiDeviceName) != name
        {
            continue;
        }
        found = true;
        let mut color = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
        color.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            size: size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        };
        let error = unsafe { DisplayConfigGetDeviceInfo(&mut color.header) };
        if error != 0 {
            return Err(format!(
                "cannot verify SDR display mode: Windows error {error}"
            ));
        }
        if unsafe { color.Anonymous.value } & 2 != 0 {
            return Err("Hardware brightness is unavailable with Windows HDR/advanced color enabled; use Shader".into());
        }
    }
    if !found {
        return Err("cannot identify the active display color mode".into());
    }
    Ok(())
}
struct Dc(HDC);
impl Drop for Dc {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.0);
        }
    }
}
fn dc(device: &Device) -> Result<Dc, String> {
    if device_identity(&device.name)? != device.identity {
        return Err("gamma recovery monitor was disconnected or replaced".into());
    }
    check_sdr(&device.name)?;
    let name = wide(&device.name);
    let handle = unsafe {
        CreateDCW(
            std::ptr::null(),
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if handle.is_null() {
        Err(os_error("open display gamma device"))
    } else {
        Ok(Dc(handle))
    }
}
pub(super) struct WindowsBackend {
    pub parent: Option<Owner>,
}
impl Backend for WindowsBackend {
    type Lease = Lease;
    fn owner(&self) -> Owner {
        own_identity().expect("guardian process identity")
    }
    fn lock(&self, device: &Device) -> Result<Lease, String> {
        monitor_lock(device)
    }
    fn window_device(&self, hwnd: usize) -> Result<Device, String> {
        let hwnd = hwnd as HWND;
        let mut pid = 0;
        if unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0
            || self.parent.is_some_and(|p| p.pid != pid)
        {
            return Err("hardware gamma window does not belong to the game".into());
        }
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) } == 0 {
            return Err(os_error("get hardware gamma monitor"));
        }
        let name = text(&info.szDevice);
        Ok(Device {
            identity: device_identity(&name)?,
            name,
        })
    }
    fn read(&self, device: &Device) -> Result<Ramp, String> {
        let dc = dc(device)?;
        let mut ramp = Ramp([0; 768]);
        if unsafe { GetDeviceGammaRamp(dc.0, ramp.0.as_mut_ptr().cast()) } == 0 {
            Err(os_error("read desktop gamma"))
        } else {
            Ok(ramp)
        }
    }
    fn write(&self, device: &Device, ramp: &Ramp) -> Result<(), String> {
        let dc = dc(device)?;
        if unsafe { SetDeviceGammaRamp(dc.0, ramp.0.as_ptr().cast()) } == 0 {
            Err(os_error("set display gamma"))
        } else {
            Ok(())
        }
    }
}
pub(super) fn replace_file(source: &Path, dest: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<_> = source.as_os_str().encode_wide().chain([0]).collect();
    let dest: Vec<_> = dest.as_os_str().encode_wide().chain([0]).collect();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            dest.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(os_error("persist gamma recovery backup"))
    } else {
        Ok(())
    }
}
