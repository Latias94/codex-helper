use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::config::proxy_home_dir;

const TOKEN_FILE_NAME: &str = "local-operator.token";
const TOKEN_PREFIX: &str = "codex-helper-local-v1-";
const NONCE_HEX_LEN: usize = 64;
const SESSION_TTL_MS: u64 = 30_000;
const MAX_CLOCK_SKEW_MS: u64 = 30_000;
const MAX_ACTIVE_SESSIONS: usize = 128;
const CLIENT_PROOF_DOMAIN: &[u8] = b"codex-helper-local-operator-client-proof-v1";
const SERVER_PROOF_DOMAIN: &[u8] = b"codex-helper-local-operator-server-proof-v1";
const REQUEST_SIGNATURE_DOMAIN: &[u8] = b"codex-helper-local-operator-request-v1";

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct LocalOperatorSessionRequest {
    pub client_nonce: String,
    pub timestamp_ms: u64,
    pub proof: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct LocalOperatorSessionResponse {
    pub session_id: String,
    pub expires_at_ms: u64,
    pub proof: String,
}

#[derive(Debug, Clone)]
struct LocalOperatorSession {
    client_nonce: String,
    issued_at_ms: u64,
    expires_at_ms: u64,
}

#[derive(Debug, Default)]
struct LocalOperatorSessionState {
    sessions: HashMap<String, LocalOperatorSession>,
    seen_client_nonces: HashMap<String, u64>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LocalOperatorSessionStore {
    state: Arc<Mutex<LocalOperatorSessionState>>,
}

impl LocalOperatorSessionStore {
    pub(crate) fn issue(
        &self,
        token: &str,
        request: &LocalOperatorSessionRequest,
    ) -> Result<LocalOperatorSessionResponse> {
        self.issue_at(token, request, unix_time_ms())
    }

    fn issue_at(
        &self,
        token: &str,
        request: &LocalOperatorSessionRequest,
        now_ms: u64,
    ) -> Result<LocalOperatorSessionResponse> {
        if !valid_nonce(&request.client_nonce) {
            anyhow::bail!("local operator client nonce is invalid");
        }
        if now_ms.abs_diff(request.timestamp_ms) > MAX_CLOCK_SKEW_MS {
            anyhow::bail!("local operator session timestamp is outside the allowed window");
        }
        verify_local_operator_client_proof(token, request)
            .context("local operator client proof is invalid")?;
        let session_id = new_local_operator_nonce();
        let expires_at_ms = now_ms.saturating_add(SESSION_TTL_MS);
        let proof =
            local_operator_server_proof(token, &request.client_nonce, &session_id, expires_at_ms)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("local operator session store is unavailable"))?;
        state
            .sessions
            .retain(|_, session| session.expires_at_ms >= now_ms);
        state
            .seen_client_nonces
            .retain(|_, expires_at_ms| *expires_at_ms >= now_ms);
        if state.seen_client_nonces.contains_key(&request.client_nonce) {
            anyhow::bail!("local operator client proof was already used");
        }
        if state.sessions.len() >= MAX_ACTIVE_SESSIONS {
            anyhow::bail!("too many local operator sessions are active");
        }
        state.seen_client_nonces.insert(
            request.client_nonce.clone(),
            request.timestamp_ms.saturating_add(MAX_CLOCK_SKEW_MS),
        );
        state.sessions.insert(
            session_id.clone(),
            LocalOperatorSession {
                client_nonce: request.client_nonce.clone(),
                issued_at_ms: now_ms,
                expires_at_ms,
            },
        );
        Ok(LocalOperatorSessionResponse {
            session_id,
            expires_at_ms,
            proof,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn verify_and_consume(
        &self,
        token: &str,
        session_id: &str,
        request_nonce: &str,
        timestamp_ms: u64,
        path: &str,
        body: &[u8],
        signature: &str,
    ) -> Result<()> {
        self.verify_and_consume_at(
            token,
            session_id,
            request_nonce,
            timestamp_ms,
            path,
            body,
            signature,
            unix_time_ms(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_and_consume_at(
        &self,
        token: &str,
        session_id: &str,
        request_nonce: &str,
        timestamp_ms: u64,
        path: &str,
        body: &[u8],
        signature: &str,
        now_ms: u64,
    ) -> Result<()> {
        if !valid_nonce(session_id) || !valid_nonce(request_nonce) {
            anyhow::bail!("local operator request nonce is invalid");
        }
        if now_ms.abs_diff(timestamp_ms) > MAX_CLOCK_SKEW_MS {
            anyhow::bail!("local operator request timestamp is outside the allowed window");
        }
        let session = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("local operator session store is unavailable"))?
            .sessions
            .remove(session_id)
            .ok_or_else(|| anyhow::anyhow!("local operator session is missing or already used"))?;
        if now_ms > session.expires_at_ms
            || timestamp_ms < session.issued_at_ms.saturating_sub(MAX_CLOCK_SKEW_MS)
            || timestamp_ms > session.expires_at_ms
        {
            anyhow::bail!("local operator session has expired");
        }
        verify_local_operator_request_signature(
            token,
            &session.client_nonce,
            session_id,
            request_nonce,
            timestamp_ms,
            path,
            body,
            signature,
        )
    }
}

pub(crate) fn new_local_operator_nonce() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub(crate) fn local_operator_client_proof(
    token: &str,
    client_nonce: &str,
    timestamp_ms: u64,
) -> Result<String> {
    mac_hex(
        token,
        CLIENT_PROOF_DOMAIN,
        &[client_nonce.as_bytes(), timestamp_ms.to_string().as_bytes()],
    )
}

fn verify_local_operator_client_proof(
    token: &str,
    request: &LocalOperatorSessionRequest,
) -> Result<()> {
    verify_mac(
        token,
        CLIENT_PROOF_DOMAIN,
        &[
            request.client_nonce.as_bytes(),
            request.timestamp_ms.to_string().as_bytes(),
        ],
        &request.proof,
    )
}

pub(crate) fn verify_local_operator_server_proof(
    token: &str,
    client_nonce: &str,
    response: &LocalOperatorSessionResponse,
) -> Result<()> {
    verify_mac(
        token,
        SERVER_PROOF_DOMAIN,
        &[
            client_nonce.as_bytes(),
            response.session_id.as_bytes(),
            response.expires_at_ms.to_string().as_bytes(),
        ],
        &response.proof,
    )
    .context("local operator daemon proof is invalid")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn local_operator_request_signature(
    token: &str,
    client_nonce: &str,
    session_id: &str,
    request_nonce: &str,
    timestamp_ms: u64,
    path: &str,
    body: &[u8],
) -> Result<String> {
    mac_hex(
        token,
        REQUEST_SIGNATURE_DOMAIN,
        &[
            client_nonce.as_bytes(),
            session_id.as_bytes(),
            request_nonce.as_bytes(),
            timestamp_ms.to_string().as_bytes(),
            b"POST",
            path.as_bytes(),
            body,
        ],
    )
}

fn local_operator_server_proof(
    token: &str,
    client_nonce: &str,
    session_id: &str,
    expires_at_ms: u64,
) -> Result<String> {
    mac_hex(
        token,
        SERVER_PROOF_DOMAIN,
        &[
            client_nonce.as_bytes(),
            session_id.as_bytes(),
            expires_at_ms.to_string().as_bytes(),
        ],
    )
}

#[allow(clippy::too_many_arguments)]
fn verify_local_operator_request_signature(
    token: &str,
    client_nonce: &str,
    session_id: &str,
    request_nonce: &str,
    timestamp_ms: u64,
    path: &str,
    body: &[u8],
    signature: &str,
) -> Result<()> {
    verify_mac(
        token,
        REQUEST_SIGNATURE_DOMAIN,
        &[
            client_nonce.as_bytes(),
            session_id.as_bytes(),
            request_nonce.as_bytes(),
            timestamp_ms.to_string().as_bytes(),
            b"POST",
            path.as_bytes(),
            body,
        ],
        signature,
    )
    .context("local operator request signature is invalid")
}

fn mac_hex(token: &str, domain: &[u8], components: &[&[u8]]) -> Result<String> {
    let mac = build_mac(token, domain, components)?;
    let bytes = mac.finalize().into_bytes();
    Ok(encode_hex(&bytes))
}

fn verify_mac(token: &str, domain: &[u8], components: &[&[u8]], encoded: &str) -> Result<()> {
    let signature = decode_hex_32(encoded)
        .ok_or_else(|| anyhow::anyhow!("local operator MAC encoding is invalid"))?;
    let mac = build_mac(token, domain, components)?;
    mac.verify_slice(&signature)
        .map_err(|_| anyhow::anyhow!("local operator MAC did not verify"))
}

fn build_mac(token: &str, domain: &[u8], components: &[&[u8]]) -> Result<HmacSha256> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(token.as_bytes())
        .map_err(|_| anyhow::anyhow!("local operator token cannot initialize HMAC"))?;
    update_mac_component(&mut mac, domain);
    for component in components {
        update_mac_component(&mut mac, component);
    }
    Ok(mac)
}

fn update_mac_component(mac: &mut HmacSha256, component: &[u8]) {
    mac.update(&(component.len() as u64).to_be_bytes());
    mac.update(component);
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_hex_32(encoded: &str) -> Option<[u8; 32]> {
    if encoded.len() != 64 {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, chunk) in encoded.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (decode_hex_nibble(chunk[0])? << 4) | decode_hex_nibble(chunk[1])?;
    }
    Some(output)
}

fn decode_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn valid_nonce(value: &str) -> bool {
    value.len() == NONCE_HEX_LEN && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub fn local_operator_token_path() -> PathBuf {
    local_operator_token_path_in(proxy_home_dir())
}

pub fn ensure_local_operator_token() -> Result<String> {
    ensure_local_operator_token_in(proxy_home_dir())
}

pub(crate) fn read_local_operator_token() -> Result<Option<String>> {
    read_local_operator_token_from(proxy_home_dir())
}

fn local_operator_token_path_in(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref().join(TOKEN_FILE_NAME)
}

enum LocalOperatorTokenFileState {
    Missing,
    Incomplete,
    Ready(String),
}

fn ensure_local_operator_token_in(home: impl AsRef<Path>) -> Result<String> {
    let home = home.as_ref();
    fs::create_dir_all(home)
        .with_context(|| format!("create codex-helper home {}", home.display()))?;
    secure_private_directory(home)?;
    let path = local_operator_token_path_in(home);
    match inspect_local_operator_token_path(&path)? {
        LocalOperatorTokenFileState::Ready(token) => {
            secure_private_file(&path)?;
            return Ok(token);
        }
        LocalOperatorTokenFileState::Incomplete => {
            return read_concurrently_created_token(&path);
        }
        LocalOperatorTokenFileState::Missing => {}
    }

    let token = format!(
        "{TOKEN_PREFIX}{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut temporary = tempfile::Builder::new()
        .prefix(".local-operator-token-")
        .suffix(".tmp")
        .tempfile_in(home)
        .with_context(|| format!("create local operator token in {}", home.display()))?;
    secure_private_file(temporary.path())?;
    temporary
        .write_all(token.as_bytes())
        .and_then(|()| temporary.write_all(b"\n"))
        .and_then(|()| temporary.as_file().sync_all())
        .with_context(|| format!("write local operator token {}", temporary.path().display()))?;
    match temporary.persist_noclobber(&path) {
        Ok(file) => {
            file.sync_all()
                .with_context(|| format!("sync local operator token {}", path.display()))?;
            secure_private_file(&path)?;
            Ok(token)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_concurrently_created_token(&path)
        }
        Err(error) => Err(error.error)
            .with_context(|| format!("publish local operator token {}", path.display())),
    }
}

fn read_concurrently_created_token(path: &Path) -> Result<String> {
    for _ in 0..20 {
        match inspect_local_operator_token_path(path)? {
            LocalOperatorTokenFileState::Ready(token) => return Ok(token),
            LocalOperatorTokenFileState::Missing | LocalOperatorTokenFileState::Incomplete => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    anyhow::bail!(
        "local operator token {} is incomplete or invalid",
        path.display()
    )
}

pub(crate) fn read_local_operator_token_from(home: impl AsRef<Path>) -> Result<Option<String>> {
    read_local_operator_token_path(&local_operator_token_path_in(home))
}

fn read_local_operator_token_path(path: &Path) -> Result<Option<String>> {
    match inspect_local_operator_token_path(path)? {
        LocalOperatorTokenFileState::Missing => Ok(None),
        LocalOperatorTokenFileState::Ready(token) => Ok(Some(token)),
        LocalOperatorTokenFileState::Incomplete => {
            anyhow::bail!("local operator token {} is invalid", path.display())
        }
    }
}

fn inspect_local_operator_token_path(path: &Path) -> Result<LocalOperatorTokenFileState> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LocalOperatorTokenFileState::Missing);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect local operator token {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!(
            "local operator token {} must be a regular file",
            path.display()
        );
    }
    #[cfg(windows)]
    {
        let information = crate::windows_file_info::path_information_no_follow(path)
            .with_context(|| format!("inspect local operator token {}", path.display()))?;
        if crate::windows_file_info::is_reparse_point(&information)
            || information.number_of_links() != 1
        {
            anyhow::bail!(
                "local operator token {} must not be a reparse point or hard link",
                path.display()
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            anyhow::bail!(
                "local operator token {} must not have hard links",
                path.display()
            );
        }
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("read local operator token {}", path.display()))?;
    let token = text.trim();
    if !valid_local_operator_token(token) {
        return Ok(LocalOperatorTokenFileState::Incomplete);
    }
    Ok(LocalOperatorTokenFileState::Ready(token.to_string()))
}

fn valid_local_operator_token(token: &str) -> bool {
    token.strip_prefix(TOKEN_PREFIX).is_some_and(|value| {
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(unix)]
fn secure_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("secure codex-helper home {}", path.display()))
}

#[cfg(windows)]
fn secure_private_directory(path: &Path) -> Result<()> {
    secure_private_windows_path(path, true)
}

#[cfg(all(not(unix), not(windows)))]
fn secure_private_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("secure local operator token {}", path.display()))
}

#[cfg(windows)]
fn secure_private_file(path: &Path) -> Result<()> {
    secure_private_windows_path(path, false)
}

#[cfg(windows)]
struct OwnedWindowsHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedWindowsHandle {
    fn drop(&mut self) {
        // SAFETY: This guard owns the non-null handle returned by OpenProcessToken.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
struct OwnedWindowsLocalAllocation(*mut core::ffi::c_void);

#[cfg(windows)]
impl Drop for OwnedWindowsLocalAllocation {
    fn drop(&mut self) {
        // SAFETY: The pointer was allocated by a Win32 API documented for LocalFree.
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0);
        }
    }
}

#[cfg(windows)]
pub fn current_windows_user_sid_string() -> Result<String> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: The output pointer is valid and the pseudo process handle needs no cleanup.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error())
            .context("open the current Windows process token");
    }
    let token = OwnedWindowsHandle(token);

    let mut required_bytes = 0_u32;
    // SAFETY: A null buffer with length zero is the documented size-query form.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut required_bytes,
        );
    }
    if required_bytes == 0 {
        return Err(std::io::Error::last_os_error())
            .context("size the current Windows user token information");
    }
    let word_size = std::mem::size_of::<usize>();
    let words = usize::try_from(required_bytes)
        .unwrap_or(usize::MAX)
        .saturating_add(word_size.saturating_sub(1))
        / word_size;
    let mut buffer = vec![0_usize; words];
    // SAFETY: The usize buffer is aligned for TOKEN_USER and has the queried byte length.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required_bytes,
            &mut required_bytes,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error())
            .context("read the current Windows user token information");
    }
    // SAFETY: GetTokenInformation initialized a TOKEN_USER at the aligned buffer start.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut string_sid = std::ptr::null_mut();
    // SAFETY: The SID belongs to the live token-information buffer and the output is valid.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut string_sid) } == 0 {
        return Err(std::io::Error::last_os_error()).context("format the current Windows user SID");
    }
    let string_sid = OwnedWindowsLocalAllocation(string_sid.cast());
    let wide = string_sid.0.cast::<u16>();
    let mut length = 0_usize;
    // SAFETY: ConvertSidToStringSidW returns a NUL-terminated LocalAlloc string.
    unsafe {
        while *wide.add(length) != 0 {
            length = length.saturating_add(1);
        }
    }
    // SAFETY: The preceding loop found the terminator within the API-owned string.
    let wide = unsafe { std::slice::from_raw_parts(wide, length) };
    String::from_utf16(wide).context("decode the current Windows user SID")
}

#[cfg(any(windows, test))]
fn windows_private_sddl(user_sid: &str, directory: bool) -> String {
    let inheritance = if directory { "OICI" } else { "" };
    format!(
        "D:P(A;{inheritance};FA;;;{user_sid})(A;{inheritance};FR;;;SY)(A;{inheritance};FR;;;BA)"
    )
}

#[cfg(windows)]
fn secure_private_windows_path(path: &Path, directory: bool) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
        SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
    };

    let user_sid = current_windows_user_sid_string()?;
    let sddl = windows_private_sddl(&user_sid, directory);
    let sddl = sddl
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: The SDDL buffer is NUL-terminated and the output pointer is valid.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("build private Windows ACL for {}", path.display()));
    }
    let descriptor = OwnedWindowsLocalAllocation(descriptor);
    let path_wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut dacl_present = 0;
    let mut dacl_defaulted = 0;
    let mut dacl = std::ptr::null_mut();
    // SAFETY: The converted descriptor remains alive and all output pointers are valid.
    if unsafe {
        GetSecurityDescriptorDacl(
            descriptor.0,
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("read private Windows ACL for {}", path.display()));
    }
    if dacl_present == 0 || dacl.is_null() {
        anyhow::bail!(
            "private Windows security descriptor for {} has no usable DACL",
            path.display()
        );
    }
    // SetFileSecurityW does not support the protected-DACL flag. Apply the complete DACL and
    // inheritance boundary together through the named-object API.
    // SAFETY: The path is NUL-terminated and the DACL points into the live descriptor.
    let status = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            dacl,
            std::ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(i32::MAX),
        ))
        .with_context(|| format!("secure Windows path {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-helper-local-operator-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn session_request(
        token: &str,
        client_nonce: impl Into<String>,
        timestamp_ms: u64,
    ) -> LocalOperatorSessionRequest {
        let client_nonce = client_nonce.into();
        let proof = local_operator_client_proof(token, &client_nonce, timestamp_ms)
            .expect("sign session request");
        LocalOperatorSessionRequest {
            client_nonce,
            timestamp_ms,
            proof,
        }
    }

    #[test]
    fn local_operator_token_is_stable_and_private() {
        let home = temp_home("stable");
        let first = ensure_local_operator_token_in(&home).expect("create token");
        let second = ensure_local_operator_token_in(&home).expect("read token");
        assert_eq!(first, second);
        assert!(valid_local_operator_token(&first));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(local_operator_token_path_in(&home))
                .expect("token metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        #[cfg(windows)]
        {
            let user_sid = current_windows_user_sid_string().expect("current Windows user SID");
            assert_eq!(
                windows_dacl_sddl(&home),
                canonical_windows_dacl_sddl(&windows_private_sddl(&user_sid, true)),
                "helper home must apply the exact private DACL"
            );
            assert_eq!(
                windows_dacl_sddl(&local_operator_token_path_in(&home)),
                canonical_windows_dacl_sddl(&windows_private_sddl(&user_sid, false)),
                "operator token must apply the exact private DACL"
            );
        }
        fs::remove_dir_all(home).expect("remove temp home");
    }

    #[cfg(windows)]
    fn canonical_windows_dacl_sddl(sddl: &str) -> String {
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };

        let sddl = sddl
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: The SDDL buffer is NUL-terminated and the output pointer is valid.
        let succeeded = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(
            succeeded,
            0,
            "parse expected Windows DACL: {}",
            std::io::Error::last_os_error()
        );
        assert!(
            !descriptor.is_null(),
            "parsed Windows DACL descriptor must not be null"
        );
        let descriptor = OwnedWindowsLocalAllocation(descriptor);
        // SAFETY: The LocalAlloc descriptor remains live for the duration of the conversion.
        unsafe { windows_descriptor_dacl_sddl(descriptor.0) }
    }

    #[cfg(windows)]
    fn windows_dacl_sddl(path: &Path) -> String {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, GetFileSecurityW};

        let path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut required_bytes = 0_u32;
        // SAFETY: A null descriptor with length zero is the documented size-query form.
        unsafe {
            GetFileSecurityW(
                path.as_ptr(),
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                0,
                &mut required_bytes,
            );
        }
        assert!(required_bytes > 0, "size Windows security descriptor");

        let word_size = std::mem::size_of::<usize>();
        let words = usize::try_from(required_bytes)
            .expect("Windows descriptor size")
            .saturating_add(word_size.saturating_sub(1))
            / word_size;
        let mut descriptor = vec![0_usize; words];
        // SAFETY: The aligned buffer has the size returned by the preceding query.
        let succeeded = unsafe {
            GetFileSecurityW(
                path.as_ptr(),
                DACL_SECURITY_INFORMATION,
                descriptor.as_mut_ptr().cast(),
                required_bytes,
                &mut required_bytes,
            )
        };
        assert_ne!(
            succeeded,
            0,
            "read Windows security descriptor: {}",
            std::io::Error::last_os_error()
        );

        // SAFETY: GetFileSecurityW initialized the descriptor in the live aligned buffer.
        unsafe { windows_descriptor_dacl_sddl(descriptor.as_mut_ptr().cast()) }
    }

    /// # Safety
    ///
    /// `descriptor` must point to a live Windows security descriptor for the duration of the call.
    #[cfg(windows)]
    unsafe fn windows_descriptor_dacl_sddl(descriptor: *mut core::ffi::c_void) -> String {
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

        let mut sddl = std::ptr::null_mut();
        // SAFETY: The caller guarantees a live descriptor and the output pointer is valid.
        let succeeded = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut sddl,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(
            succeeded,
            0,
            "format Windows DACL: {}",
            std::io::Error::last_os_error()
        );
        assert!(!sddl.is_null(), "formatted Windows DACL must not be null");
        let sddl = OwnedWindowsLocalAllocation(sddl.cast());
        let wide = sddl.0.cast::<u16>();
        let mut length = 0_usize;
        // SAFETY: The conversion API returned a NUL-terminated LocalAlloc string.
        unsafe {
            while *wide.add(length) != 0 {
                length = length.saturating_add(1);
            }
        }
        // SAFETY: The preceding loop found the terminator within the live API-owned string.
        String::from_utf16(unsafe { std::slice::from_raw_parts(wide, length) })
            .expect("decode Windows DACL SDDL")
    }

    #[test]
    fn stale_unpublished_token_file_does_not_block_initialization() {
        let home = temp_home("stale-temp");
        fs::create_dir_all(&home).expect("create home");
        fs::write(home.join(".local-operator-token-stale.tmp"), b"incomplete")
            .expect("write stale temp file");

        let token = ensure_local_operator_token_in(&home).expect("create published token");

        assert!(valid_local_operator_token(&token));
        assert_eq!(
            read_local_operator_token_from(&home)
                .expect("read token")
                .as_deref(),
            Some(token.as_str())
        );
        fs::remove_dir_all(home).expect("remove temp home");
    }

    #[cfg(unix)]
    #[test]
    fn local_operator_token_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let home = temp_home("symlink");
        fs::create_dir_all(&home).expect("create home");
        let target = home.join("target");
        fs::write(&target, format!("{TOKEN_PREFIX}{}\n", "a".repeat(64))).expect("write target");
        symlink(&target, local_operator_token_path_in(&home)).expect("create symlink");
        let error = ensure_local_operator_token_in(&home).expect_err("reject symlink");
        assert!(error.to_string().contains("regular file"));
        fs::remove_dir_all(home).expect("remove temp home");
    }

    #[cfg(unix)]
    #[test]
    fn local_operator_token_rejects_hard_links() {
        let home = temp_home("hard-link");
        fs::create_dir_all(&home).expect("create home");
        let path = local_operator_token_path_in(&home);
        fs::write(&path, format!("{TOKEN_PREFIX}{}\n", "a".repeat(64))).expect("write token");
        fs::hard_link(&path, home.join("token-copy")).expect("create hard link");

        let error = ensure_local_operator_token_in(&home).expect_err("reject hard link");

        assert!(error.to_string().contains("hard links"));
        fs::remove_dir_all(home).expect("remove temp home");
    }

    #[test]
    fn daemon_proof_and_one_time_request_signature_round_trip() {
        let token = format!("{TOKEN_PREFIX}{}", "a".repeat(64));
        let client_nonce = "b".repeat(64);
        let request = session_request(&token, client_nonce.clone(), 1_000);
        let sessions = LocalOperatorSessionStore::default();
        let session = sessions
            .issue_at(&token, &request, 1_000)
            .expect("issue session");
        verify_local_operator_server_proof(&token, &client_nonce, &session)
            .expect("verify daemon proof");
        let request_nonce = "c".repeat(64);
        let body = br#"{"force":true}"#;
        let signature = local_operator_request_signature(
            &token,
            &client_nonce,
            &session.session_id,
            &request_nonce,
            1_001,
            "/action",
            body,
        )
        .expect("sign request");

        sessions
            .verify_and_consume_at(
                &token,
                &session.session_id,
                &request_nonce,
                1_001,
                "/action",
                body,
                &signature,
                1_001,
            )
            .expect("verify request");
        assert!(
            sessions
                .verify_and_consume_at(
                    &token,
                    &session.session_id,
                    &request_nonce,
                    1_001,
                    "/action",
                    body,
                    &signature,
                    1_001,
                )
                .is_err(),
            "a local operator session must be single-use"
        );
    }

    #[test]
    fn request_signature_binds_the_path_and_body() {
        let token = format!("{TOKEN_PREFIX}{}", "d".repeat(64));
        let client_nonce = "e".repeat(64);
        let request = session_request(&token, client_nonce.clone(), 10_000);
        let sessions = LocalOperatorSessionStore::default();
        let session = sessions
            .issue_at(&token, &request, 10_000)
            .expect("issue session");
        let request_nonce = "f".repeat(64);
        let signature = local_operator_request_signature(
            &token,
            &client_nonce,
            &session.session_id,
            &request_nonce,
            10_001,
            "/routing",
            br#"{"target":"a"}"#,
        )
        .expect("sign request");

        assert!(
            sessions
                .verify_and_consume_at(
                    &token,
                    &session.session_id,
                    &request_nonce,
                    10_001,
                    "/routing",
                    br#"{"target":"b"}"#,
                    &signature,
                    10_001,
                )
                .is_err()
        );
    }

    #[test]
    fn session_issue_rejects_missing_client_proof_without_consuming_capacity() {
        let token = format!("{TOKEN_PREFIX}{}", "1".repeat(64));
        let sessions = LocalOperatorSessionStore::default();
        for index in 0..=MAX_ACTIVE_SESSIONS {
            let request = LocalOperatorSessionRequest {
                client_nonce: format!("{index:064x}"),
                timestamp_ms: 20_000,
                proof: "0".repeat(64),
            };
            assert!(sessions.issue_at(&token, &request, 20_000).is_err());
        }

        let valid = session_request(&token, "2".repeat(64), 20_000);
        sessions
            .issue_at(&token, &valid, 20_000)
            .expect("invalid proofs must not consume session capacity");
    }

    #[test]
    fn session_issue_rejects_replayed_client_proof() {
        let token = format!("{TOKEN_PREFIX}{}", "3".repeat(64));
        let sessions = LocalOperatorSessionStore::default();
        let request = session_request(&token, "4".repeat(64), 30_000);

        sessions
            .issue_at(&token, &request, 30_000)
            .expect("first client proof");
        let error = sessions
            .issue_at(&token, &request, 30_001)
            .expect_err("replayed client proof");

        assert!(error.to_string().contains("already used"), "{error:#}");
    }

    #[test]
    fn windows_private_acl_is_protected_and_exact() {
        let file = windows_private_sddl("S-1-5-21-100-200-300-400", false);
        let directory = windows_private_sddl("S-1-5-21-100-200-300-400", true);

        assert_eq!(
            file,
            "D:P(A;;FA;;;S-1-5-21-100-200-300-400)(A;;FR;;;SY)(A;;FR;;;BA)"
        );
        assert_eq!(
            directory,
            "D:P(A;OICI;FA;;;S-1-5-21-100-200-300-400)(A;OICI;FR;;;SY)(A;OICI;FR;;;BA)"
        );
        assert!(!file.contains("WD"));
        assert!(!file.contains("AU"));
    }

    #[test]
    fn concurrent_token_initialization_returns_one_stable_secret() {
        let home = temp_home("concurrent");
        let handles = (0..8)
            .map(|_| {
                let home = home.clone();
                std::thread::spawn(move || ensure_local_operator_token_in(home))
            })
            .collect::<Vec<_>>();
        let tokens = handles
            .into_iter()
            .map(|handle| handle.join().expect("join token creator").expect("token"))
            .collect::<Vec<_>>();

        assert!(tokens.windows(2).all(|pair| pair[0] == pair[1]));
        fs::remove_dir_all(home).expect("remove temp home");
    }
}
