use crate::secret;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use std::path::Path;
fn key(path: &Path) -> Result<SigningKey, String> {
    if path.exists() {
        let bytes = secret::load(path)?;
        SigningKey::from_slice(&bytes)
            .map_err(|_| "端末鍵を読み取れません。新しいPCとして再登録してください".into())
    } else {
        let key = loop {
            if let Ok(key) = SigningKey::from_slice(&rand::random::<[u8; 32]>()) {
                break key;
            }
        };
        secret::save(path, &key.to_bytes())?;
        Ok(key)
    }
}
pub fn public(path: &Path) -> Result<String, String> {
    Ok(URL_SAFE_NO_PAD.encode(
        key(path)?
            .verifying_key()
            .to_encoded_point(false)
            .as_bytes(),
    ))
}
pub fn sign(path: &Path, challenge: &str) -> Result<String, String> {
    let sig: Signature = key(path)?.sign(challenge.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(sig.to_bytes()))
}
#[cfg(windows)]
pub fn verify_user(window: isize) -> Result<(), String> {
    use windows::{
        core::{factory, HSTRING},
        Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier},
        Win32::{
            Foundation::HWND,
            System::WinRT::{
                IUserConsentVerifierInterop, RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
            },
        },
    };
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED).map_err(|_| "Windows本人確認を初期化できません")?;
        let result = (|| -> windows::core::Result<bool> {
            let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
            let operation: windows_future::IAsyncOperation<UserConsentVerificationResult> = interop
                .RequestVerificationForWindowAsync(
                    HWND(window as *mut _),
                    &HSTRING::from("Mouse Stats のアカウントを復旧します"),
                )?;
            Ok(operation.join()? == UserConsentVerificationResult::Verified)
        })();
        RoUninitialize();
        if result.unwrap_or(false) {
            Ok(())
        } else {
            Err("Windows Helloでの本人確認が完了しませんでした。Passkeyまたは復旧コードを使用してください".into())
        }
    }
}
#[cfg(not(windows))]
pub fn verify_user(_: isize) -> Result<(), String> {
    Err("Windows Hello required".into())
}
