#[cfg(windows)]
mod platform {
    use core::ffi::c_void;
    use std::{
        os::windows::ffi::OsStrExt,
        path::Path,
        ptr, slice, thread,
        time::Duration,
    };

    type Handle = *mut c_void;
    const CF_DIB: u32 = 8;
    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const DROPEFFECT_COPY: u32 = 1;
    const GMEM_MOVEABLE: u32 = 0x0002;
    const BI_RGB: u32 = 0;

    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_pels_per_meter: i32,
        y_pels_per_meter: i32,
        clr_used: u32,
        clr_important: u32,
    }

    /// Header of the CF_HDROP payload (Win32 `DROPFILES`); the file list follows it.
    #[repr(C)]
    struct DropFiles {
        files_offset: u32,
        pt_x: i32,
        pt_y: i32,
        non_client: i32,
        wide: i32,
    }

    #[link(name = "User32")]
    extern "system" {
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
        fn OpenClipboard(hwnd_new_owner: Handle) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn GetClipboardData(format: u32) -> Handle;
        fn SetClipboardData(format: u32, memory: Handle) -> Handle;
    }

    #[link(name = "Kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
        fn GlobalFree(memory: Handle) -> Handle;
        fn GlobalLock(memory: Handle) -> *mut c_void;
        fn GlobalUnlock(memory: Handle) -> i32;
    }

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    fn open() -> Result<ClipboardGuard, String> {
        if unsafe { OpenClipboard(ptr::null_mut()) } == 0 {
            return Err("Could not open the Windows clipboard".into());
        }
        Ok(ClipboardGuard)
    }

    fn open_with_retry() -> Result<ClipboardGuard, String> {
        const ATTEMPTS: usize = 8;
        for attempt in 0..ATTEMPTS {
            if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
                return Ok(ClipboardGuard);
            }
            if attempt + 1 != ATTEMPTS {
                thread::sleep(Duration::from_millis(4));
            }
        }
        Err("Could not open the Windows clipboard".into())
    }

    pub fn set_text(text: &str) -> Result<(), String> {
        let _guard = open()?;
        if unsafe { EmptyClipboard() } == 0 {
            return Err("Could not clear the Windows clipboard".into());
        }
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * std::mem::size_of::<u16>();
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
        if memory.is_null() {
            return Err("Could not allocate clipboard memory".into());
        }
        let target = unsafe { GlobalLock(memory) } as *mut u16;
        if target.is_null() {
            unsafe {
                GlobalFree(memory);
            }
            return Err("Could not lock clipboard memory".into());
        }
        unsafe {
            ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
            GlobalUnlock(memory);
        }
        if unsafe { SetClipboardData(CF_UNICODETEXT, memory) }.is_null() {
            unsafe {
                GlobalFree(memory);
            }
            return Err("Could not set clipboard text".into());
        }
        // SetClipboardData owns memory after success.
        Ok(())
    }

    /// Copies a top-down 32-bit screenshot into the Windows clipboard as CF_DIB.
    /// `pixels` may have a padded row stride and may be BGRA or RGBA.
    ///
    /// When `file` is given it is also offered as a file drop (CF_HDROP), so chat
    /// apps paste that file (e.g. a JPEG) instead of re-encoding the bitmap.
    /// The file drop is best-effort; the bitmap alone still counts as success.
    pub fn set_image_rgba8(
        width: u32,
        height: u32,
        pixels: &[u8],
        bytes_per_row: usize,
        source_is_bgra: bool,
        file: Option<&Path>,
    ) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Err("Clipboard image dimensions must be non-zero".into());
        }
        let width_i32 = i32::try_from(width)
            .map_err(|_| "Clipboard image width exceeds Windows DIB limits".to_owned())?;
        let height_i32 = i32::try_from(height)
            .map_err(|_| "Clipboard image height exceeds Windows DIB limits".to_owned())?;
        let row_bytes = (width as usize)
            .checked_mul(4)
            .ok_or_else(|| "Clipboard image row size overflowed usize".to_owned())?;
        if bytes_per_row < row_bytes {
            return Err("Clipboard image stride is smaller than its visible row".into());
        }
        let required = bytes_per_row
            .checked_mul(height as usize)
            .ok_or_else(|| "Clipboard image byte size overflowed usize".to_owned())?;
        if pixels.len() < required {
            return Err("Clipboard image source buffer is too small".into());
        }
        let image_bytes = row_bytes
            .checked_mul(height as usize)
            .ok_or_else(|| "Clipboard image byte size overflowed usize".to_owned())?;
        let size_image = u32::try_from(image_bytes)
            .map_err(|_| "Clipboard image is too large for a Windows DIB".to_owned())?;
        let header_bytes = std::mem::size_of::<BitmapInfoHeader>();
        let allocation_bytes = header_bytes
            .checked_add(image_bytes)
            .ok_or_else(|| "Clipboard image allocation size overflowed usize".to_owned())?;

        let _guard = open_with_retry()?;
        if unsafe { EmptyClipboard() } == 0 {
            return Err("Could not clear the Windows clipboard".into());
        }

        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, allocation_bytes) };
        if memory.is_null() {
            return Err("Could not allocate clipboard image memory".into());
        }
        let target = unsafe { GlobalLock(memory) } as *mut u8;
        if target.is_null() {
            unsafe {
                GlobalFree(memory);
            }
            return Err("Could not lock clipboard image memory".into());
        }

        let header = BitmapInfoHeader {
            size: header_bytes as u32,
            width: width_i32,
            // Positive DIB height means bottom-up rows, so copy them reversed below.
            height: height_i32,
            planes: 1,
            bit_count: 32,
            compression: BI_RGB,
            size_image,
            x_pels_per_meter: 0,
            y_pels_per_meter: 0,
            clr_used: 0,
            clr_important: 0,
        };

        unsafe {
            ptr::write(target.cast::<BitmapInfoHeader>(), header);
            let dst_pixels = target.add(header_bytes);
            for dst_row in 0..height as usize {
                let src_row = height as usize - 1 - dst_row;
                let src = pixels.as_ptr().add(src_row * bytes_per_row);
                let dst = dst_pixels.add(dst_row * row_bytes);
                if source_is_bgra {
                    ptr::copy_nonoverlapping(src, dst, row_bytes);
                    // DIB alpha is not meaningful with BI_RGB. Keep it opaque for
                    // consumers that nevertheless inspect the fourth byte.
                    for x in 0..width as usize {
                        *dst.add(x * 4 + 3) = 0xff;
                    }
                } else {
                    for x in 0..width as usize {
                        let src_px = src.add(x * 4);
                        let dst_px = dst.add(x * 4);
                        *dst_px.add(0) = *src_px.add(2);
                        *dst_px.add(1) = *src_px.add(1);
                        *dst_px.add(2) = *src_px.add(0);
                        *dst_px.add(3) = 0xff;
                    }
                }
            }
            GlobalUnlock(memory);
        }

        if unsafe { SetClipboardData(CF_DIB, memory) }.is_null() {
            unsafe {
                GlobalFree(memory);
            }
            return Err("Could not set clipboard image".into());
        }
        // SetClipboardData owns memory after success.
        if let Some(file) = file {
            set_file_drop(file);
        }
        Ok(())
    }

    /// Copies `bytes` into movable global memory and hands it to the open
    /// clipboard under `format`. Returns false (freeing the memory) on failure.
    fn set_global_bytes(format: u32, bytes: &[u8]) -> bool {
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if memory.is_null() {
                return false;
            }
            let target = GlobalLock(memory) as *mut u8;
            if target.is_null() {
                GlobalFree(memory);
                return false;
            }
            ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
            GlobalUnlock(memory);
            if SetClipboardData(format, memory).is_null() {
                GlobalFree(memory);
                return false;
            }
        }
        true
    }

    /// Adds `file` to the already-open clipboard as a copy-style file drop.
    fn set_file_drop(file: &Path) {
        let header = DropFiles {
            files_offset: std::mem::size_of::<DropFiles>() as u32,
            pt_x: 0,
            pt_y: 0,
            non_client: 0,
            wide: 1,
        };
        let mut payload = Vec::new();
        // SAFETY: DropFiles is repr(C) plain data with no padding.
        payload.extend_from_slice(unsafe {
            slice::from_raw_parts(
                (&header as *const DropFiles).cast::<u8>(),
                std::mem::size_of::<DropFiles>(),
            )
        });
        // Wide path, NUL-terminated, plus the list's own terminating NUL.
        for unit in file.as_os_str().encode_wide().chain([0, 0]) {
            payload.extend_from_slice(&unit.to_le_bytes());
        }
        if !set_global_bytes(CF_HDROP, &payload) {
            return;
        }
        // Tell Explorer-style consumers this is a copy, not a move.
        let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
        let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
        if format != 0 {
            set_global_bytes(format, &DROPEFFECT_COPY.to_le_bytes());
        }
    }

    pub fn get_text() -> Result<String, String> {
        let _guard = open()?;
        let memory = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if memory.is_null() {
            return Err("Clipboard does not contain text".into());
        }
        let source = unsafe { GlobalLock(memory) } as *const u16;
        if source.is_null() {
            return Err("Could not lock clipboard text".into());
        }
        let mut len = 0usize;
        unsafe {
            while *source.add(len) != 0 {
                len += 1;
            }
        }
        let text = String::from_utf16_lossy(unsafe { slice::from_raw_parts(source, len) });
        unsafe {
            GlobalUnlock(memory);
        }
        Ok(text)
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn set_text(_text: &str) -> Result<(), String> {
        Err("Clipboard integration is currently implemented for Windows only".into())
    }
    pub fn set_image_rgba8(
        _width: u32,
        _height: u32,
        _pixels: &[u8],
        _bytes_per_row: usize,
        _source_is_bgra: bool,
        _file: Option<&std::path::Path>,
    ) -> Result<(), String> {
        Err("Clipboard image integration is currently implemented for Windows only".into())
    }
    pub fn get_text() -> Result<String, String> {
        Err("Clipboard integration is currently implemented for Windows only".into())
    }
}

pub use platform::{get_text, set_image_rgba8, set_text};
