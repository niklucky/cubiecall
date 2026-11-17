//! Where the session token is kept: the Keychain on an iPhone.
//!
//! One entry per server, so a build pointed at a test server never touches
//! the real session. Only on iOS: built for the Mac (checks, tests), this
//! crate keeps nothing, and never goes near the Mac's Keychain.

const SERVICE: &str = "com.cubiecall.session";

fn account(server: &str) -> &str {
    server.trim_end_matches('/')
}

#[cfg(target_os = "ios")]
pub(crate) fn load(server: &str) -> Option<String> {
    security_framework::passwords::get_generic_password(SERVICE, account(server))
        .ok()
        .and_then(|v| String::from_utf8(v).ok())
}

#[cfg(target_os = "ios")]
pub(crate) fn save(server: &str, token: &str) -> anyhow::Result<()> {
    security_framework::passwords::set_generic_password(
        SERVICE,
        account(server),
        token.as_bytes(),
    )?;
    Ok(())
}

#[cfg(target_os = "ios")]
pub(crate) fn clear(server: &str) {
    let _ = security_framework::passwords::delete_generic_password(SERVICE, account(server));
}

// Android will keep it in the Keystore, from Kotlin: not built yet.
#[cfg(not(target_os = "ios"))]
pub(crate) fn load(_server: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "ios"))]
pub(crate) fn save(_server: &str, _token: &str) -> anyhow::Result<()> {
    let _ = (SERVICE, account);
    anyhow::bail!("there is nowhere safe to keep the session on this platform yet")
}

#[cfg(not(target_os = "ios"))]
pub(crate) fn clear(_server: &str) {}
