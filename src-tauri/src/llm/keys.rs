//! API Key 儲存:Windows Credential Manager(經 keyring crate)。
//! 規格 §8:金鑰不落明文,設定匯出不含金鑰。

use keyring::Entry;

const SERVICE: &str = "desktop-pet-ai";

pub fn set_key(provider_id: &str, key: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, provider_id).map_err(|e| e.to_string())?;
    if key.trim().is_empty() {
        // 空字串視為刪除
        let _ = entry.delete_credential();
        return Ok(());
    }
    entry.set_password(key.trim()).map_err(|e| e.to_string())
}

pub fn get_key(provider_id: &str) -> Option<String> {
    Entry::new(SERVICE, provider_id).ok()?.get_password().ok()
}

pub fn has_key(provider_id: &str) -> bool {
    get_key(provider_id).is_some()
}
