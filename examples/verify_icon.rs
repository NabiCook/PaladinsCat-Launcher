//! CPU-only GUI-subsystem/icon check. Read PE data, never execute the GUI.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};
    type Handle = *mut c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(path: *const u16, file: Handle, flags: u32) -> Handle;
        fn FreeLibrary(module: Handle) -> i32;
        fn FindResourceW(module: Handle, name: *const u16, kind: *const u16) -> Handle;
        fn SizeofResource(module: Handle, resource: Handle) -> u32;
        fn LoadResource(module: Handle, resource: Handle) -> Handle;
        fn LockResource(resource: Handle) -> Handle;
    }
    struct Module(Handle);
    impl Drop for Module {
        fn drop(&mut self) {
            unsafe { FreeLibrary(self.0) };
        }
    }
    fn resource(
        module: &Module,
        id: u16,
        kind: u16,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let resource = unsafe { FindResourceW(module.0, id as _, kind as _) };
        if resource.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let size = unsafe { SizeofResource(module.0, resource) } as usize;
        let loaded = unsafe { LoadResource(module.0, resource) };
        if loaded.is_null() || size == 0 || size > 1024 * 1024 {
            return Err("Invalid icon resource".into());
        }
        let address = unsafe { LockResource(loaded) };
        if address.is_null() {
            return Err("Icon resource cannot be read".into());
        }
        Ok(unsafe { std::slice::from_raw_parts(address.cast::<u8>(), size) }.to_vec())
    }
    let path = std::env::args_os()
        .nth(1)
        .ok_or("Pass a built launcher EXE")?;
    let bytes = std::fs::read(&path)?;
    let pe = bytes.get(0x3c..0x40).ok_or("Missing DOS header")?;
    let pe = u32::from_le_bytes(pe.try_into()?) as usize;
    if bytes.get(..2) != Some(b"MZ")
        || bytes.get(pe..pe.saturating_add(4)) != Some(b"PE\0\0")
        || bytes.get(pe.saturating_add(92)..pe.saturating_add(94)) != Some(&[2, 0])
    {
        return Err("Launcher must use the Windows GUI subsystem (2), never console (3)".into());
    }
    let path: Vec<u16> = path.encode_wide().chain([0]).collect();
    // LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE: no entry point.
    let module = unsafe { LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), 0x22) };
    if module.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let module = Module(module);
    let group = resource(&module, 1, 14)?;
    if group.len() != 20 || group[..6] != [0, 0, 1, 0, 1, 0] {
        return Err("Expected the single PaladinsCat application icon".into());
    }
    let id = u16::from_le_bytes(group[18..20].try_into()?);
    let payload = resource(&module, id, 3)?;
    let logo = include_bytes!("../assets/paladinscat.png");
    if payload != logo {
        return Err("Executable icon differs from the existing PaladinsCat logo".into());
    }
    println!(
        "PaladinsCat GUI subsystem 2 and icon verified: exact source PNG, {} bytes; CPU-only",
        payload.len()
    );
    Ok(())
}

#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("PE icon verification requires Windows".into())
}
