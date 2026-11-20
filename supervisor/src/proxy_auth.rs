use ring::{
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io,
    num::NonZeroU32,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const DEFAULT_INSTANCE: &str = "default";
pub const COOKIE_PREFIX: &str = "qwenpaw-proxy-";
const ROUNDS: NonZeroU32 = NonZeroU32::new(600_000).unwrap();
pub const TTL: Duration = Duration::from_secs(8 * 3600);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Verifier {
    salt: [u8; 16],
    hash: [u8; 32],
}
#[derive(Default)]
struct State {
    verifier: Option<Verifier>,
    unavailable: bool,
    sessions: VecDeque<(String, Instant)>,
    last_attempt: Option<Instant>,
}
pub struct ProxyAuth {
    file: String,
    pub cookie: String,
    state: Mutex<State>,
    pub rotated: tokio::sync::watch::Sender<()>,
}
fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| io::Error::other("random source unavailable"))?;
    Ok(bytes)
}
fn read_verifier(path: &Path) -> io::Result<Verifier> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.mode() & 0o077 != 0 || meta.len() > 2048 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(serde_json::from_reader(file.take(2049))?)
}
impl ProxyAuth {
    pub fn load(var: &Path, instance: &'static str) -> Arc<Self> {
        // Instance IDs are chosen by the supervisor, never from requests or port numbers.
        assert!(
            !instance.is_empty()
                && instance
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        );
        let file = var.join(format!("proxy-password-{instance}.v1"));
        let (verifier, unavailable) = match read_verifier(&file) {
            Ok(v) => (Some(v), false),
            Err(e) => (None, e.kind() != io::ErrorKind::NotFound),
        };
        Arc::new(Self {
            rotated: tokio::sync::watch::channel(()).0,
            file: file.to_string_lossy().into_owned(),
            cookie: format!("{COOKIE_PREFIX}{instance}"),
            state: Mutex::new(State {
                verifier,
                unavailable,
                ..State::default()
            }),
        })
    }
    pub fn password_status(&self) -> Option<bool> {
        let state = self.state.lock().unwrap();
        (!state.unavailable).then_some(state.verifier.is_some())
    }
    pub fn clear_password(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.unavailable {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let path = Path::new(&self.file);
        let current = read_verifier(path)?;
        if state.verifier.as_ref() != Some(&current) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let parent = std::fs::File::open(path.parent().unwrap())?;
        std::fs::remove_file(path)?;
        parent.sync_all()?;
        state.verifier = None;
        state.sessions.clear();
        state.last_attempt = None;
        self.rotated.send_replace(());
        Ok(())
    }
    #[cfg(test)]
    pub fn set_password(&self, password: &str) -> io::Result<()> {
        if !(6..=12).contains(&password.chars().count()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut record = Verifier {
            salt: random()?,
            hash: [0; 32],
        };
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            ROUNDS,
            &record.salt,
            password.as_bytes(),
            &mut record.hash,
        );
        let encoded = serde_json::to_string(&record)?;
        let written = crate::install::atomic_write_0600(&self.file, &encoded);
        let mut state = self.state.lock().unwrap();
        self.rotated.send_replace(());
        if let Err(err) = written {
            state.sessions.clear();
            state.verifier = None;
            state.unavailable = true;
            return Err(err);
        }
        state.unavailable = false;
        state.verifier = Some(record);
        state.sessions.clear();
        state.last_attempt = None;
        Ok(())
    }
    pub fn accepts(&self, token: &str) -> bool {
        token.len() == 64
            && self
                .state
                .lock()
                .unwrap()
                .sessions
                .iter()
                .any(|(stored, issued)| stored == token && issued.elapsed() < TTL)
    }
    pub async fn login(
        self: &Arc<Self>,
        password: String,
    ) -> Result<String, axum::http::StatusCode> {
        use axum::http::StatusCode as S;
        let record = {
            let mut state = self.state.lock().unwrap();
            let verifier = state.verifier.clone().ok_or(S::SERVICE_UNAVAILABLE)?;
            if state
                .last_attempt
                .is_some_and(|time| time.elapsed() < Duration::from_secs(1))
            {
                return Err(S::TOO_MANY_REQUESTS);
            }
            state.last_attempt = Some(Instant::now());
            verifier
        };
        let check = record.clone();
        let valid = tokio::task::spawn_blocking(move || {
            password.len() <= 256
                && pbkdf2::verify(
                    pbkdf2::PBKDF2_HMAC_SHA256,
                    ROUNDS,
                    &check.salt,
                    password.as_bytes(),
                    &check.hash,
                )
                .is_ok()
        })
        .await
        .unwrap_or(false);
        let mut state = self.state.lock().unwrap();
        if !valid || state.verifier.as_ref() != Some(&record) {
            return Err(S::UNAUTHORIZED);
        }
        let token: String = random::<32>()
            .map_err(|_| S::SERVICE_UNAVAILABLE)?
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if state.sessions.len() >= 64 {
            state.sessions.pop_front();
        }
        state.sessions.push_back((token.clone(), Instant::now()));
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::TestDir;
    use std::os::unix::fs::PermissionsExt;
    #[tokio::test]
    async fn new_password_limits_do_not_lock_out_existing_passwords() {
        let dir = TestDir::new("proxy-password-policy");
        let auth = ProxyAuth::load(&dir.0, DEFAULT_INSTANCE);
        let legacy = "existing long password";
        let mut record = Verifier {
            salt: [1; 16],
            hash: [0; 32],
        };
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            ROUNDS,
            &record.salt,
            legacy.as_bytes(),
            &mut record.hash,
        );
        crate::install::atomic_write_0600(&auth.file, &serde_json::to_string(&record).unwrap())
            .unwrap();
        let auth = ProxyAuth::load(&dir.0, DEFAULT_INSTANCE);
        let session = auth.login(legacy.into()).await.unwrap();
        for invalid in ["12345", "1234567890123"] {
            assert!(auth.set_password(invalid).is_err());
            assert!(auth.accepts(&session));
        }
        for valid in ["六个字符🔒好", "123456789012"] {
            auth.set_password(valid).unwrap();
            assert!(auth.login(valid.into()).await.is_ok());
        }
    }

    #[tokio::test]
    async fn sessions_and_passwords_belong_to_stable_instances() {
        let dir = TestDir::new("instance-proxy-passwords");
        let a = ProxyAuth::load(&dir.0, DEFAULT_INSTANCE);
        let b = ProxyAuth::load(&dir.0, "second");
        assert_eq!(a.password_status(), Some(false));
        assert!(a.set_password("short").is_err());
        a.set_password("fixture-A").unwrap();
        b.set_password("fixture-B").unwrap();
        let sa = a.login("fixture-A".into()).await.unwrap();
        let sb = b.login("fixture-B".into()).await.unwrap();
        assert!(a.accepts(&sa) && b.accepts(&sb));
        assert!(!b.accepts(&sa) && !a.accepts(&sb));
        a.set_password("new-fixtureA").unwrap();
        assert!(!a.accepts(&sa) && b.accepts(&sb));
        assert!(a.login("fixture-A".into()).await.is_err());
        let reopened = ProxyAuth::load(&dir.0, DEFAULT_INSTANCE);
        assert!(reopened.login("new-fixtureA".into()).await.is_ok());
        let bytes = std::fs::read_to_string(&a.file).unwrap();
        assert!(!bytes.contains("fixture"));
        assert_eq!(
            std::fs::metadata(&a.file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::write(&a.file, "truncated").unwrap();
        let corrupt = ProxyAuth::load(&dir.0, DEFAULT_INSTANCE);
        assert_eq!(corrupt.password_status(), None);
        assert!(!corrupt.accepts(&sa));
        assert!(corrupt.clear_password().is_err());
        b.clear_password().unwrap();
        assert!(!b.accepts(&sb));
        assert_eq!(
            ProxyAuth::load(&dir.0, "second").password_status(),
            Some(false)
        );
    }
}
