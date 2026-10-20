use std::path::Path;
#[cfg(windows)]
pub fn save(path: &Path, value: &[u8]) -> Result<(), String> {
    use windows::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: value.len() as u32,
            pbData: value.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| e.to_string())?;
        let data = std::slice::from_raw_parts(out.pbData, out.cbData as usize);
        let tmp = path.with_extension("tmp");
        use std::io::Write;
        let result = std::fs::File::create(&tmp)
            .and_then(|mut file| {
                file.write_all(data)?;
                file.sync_all()
            })
            .and_then(|_| std::fs::rename(&tmp, path))
            .map_err(|e| e.to_string());
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(out.pbData.cast())));
        result
    }
}
#[cfg(windows)]
pub fn load(path: &Path) -> Result<Vec<u8>, String> {
    use windows::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| e.to_string())?;
        let result = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(out.pbData.cast())));
        Ok(result)
    }
}
#[cfg(not(windows))]
pub fn save(_: &Path, _: &[u8]) -> Result<(), String> {
    Err("Windows DPAPI required".into())
}
#[cfg(not(windows))]
pub fn load(_: &Path) -> Result<Vec<u8>, String> {
    Err("Windows DPAPI required".into())
}
